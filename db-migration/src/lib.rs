//! TODO(doc): @quake
use ckb_db::{ReadOnlyDB, RocksDB};
use ckb_db_schema::{COLUMN_META, META_TIP_HEADER_KEY, MIGRATION_VERSION_KEY};
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{debug, error, info};
use console::Term;
pub use indicatif::{HumanDuration, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;

#[cfg(test)]
mod tests;

fn internal_error(reason: String) -> Error {
    InternalErrorKind::Database.other(reason).into()
}

/// TODO(doc): @quake
#[derive(Default)]
pub struct Migrations {
    migrations: BTreeMap<String, Arc<dyn Migration>>,
}

pub struct MigrationWorker {
    tasks: Arc<Mutex<Vec<(String, Arc<dyn Migration>)>>>,
    db: RocksDB,
}

impl MigrationWorker {
    pub fn new(tasks: Arc<Mutex<Vec<(String, Arc<dyn Migration>)>>>, db: RocksDB) -> Self {
        Self { tasks, db }
    }

    pub fn start(&self) {
        let db = self.db.clone();
        let tasks = self.tasks.clone();
        thread::spawn(move || loop {
            // Progress Bar is no need in background, but here we fake one
            let db = db.clone();
            let pb = move |_count: u64| -> ProgressBar { ProgressBar::new(0) };
            if let Some((_, task)) = tasks.lock().unwrap().pop() {
                let db = task.migrate(db, Arc::new(pb)).unwrap();
                db.put_default(MIGRATION_VERSION_KEY, task.version())
                    .map_err(|err| internal_error(format!("failed to migrate the database: {err}")))
                    .unwrap();
            } else {
                break;
            }
        });
    }
}

impl Migrations {
    /// TODO(doc): @quake
    pub fn new() -> Self {
        Migrations {
            migrations: BTreeMap::new(),
        }
    }

    /// TODO(doc): @quake
    pub fn add_migration(&mut self, migration: Arc<dyn Migration>) {
        self.migrations
            .insert(migration.version().to_string(), migration);
    }

    /// Check if database's version is matched with the executable binary version.
    ///
    /// Returns
    /// - Less: The database version is less than the matched version of the executable binary.
    ///   Requires migration.
    /// - Equal: The database version is matched with the executable binary version.
    /// - Greater: The database version is greater than the matched version of the executable binary.
    ///   Requires upgrade the executable binary.
    pub fn check(&self, db: &ReadOnlyDB) -> Ordering {
        let db_version = match db
            .get_pinned_default(MIGRATION_VERSION_KEY)
            .expect("get the version of database")
        {
            Some(version_bytes) => {
                String::from_utf8(version_bytes.to_vec()).expect("version bytes to utf8")
            }
            None => {
                // if version is none, but db is not empty
                // patch 220464f
                if self.is_non_empty_rdb(db) {
                    return Ordering::Less;
                } else {
                    return Ordering::Equal;
                }
            }
        };
        debug!("current database version [{}]", db_version);

        let latest_version = self
            .migrations
            .values()
            .last()
            .unwrap_or_else(|| panic!("should have at least one version"))
            .version();
        debug!("latest  database version [{}]", latest_version);

        db_version.as_str().cmp(latest_version)
    }

    /// Check if the migrations will consume a lot of time.
    pub fn expensive(&self, db: &ReadOnlyDB) -> bool {
        let db_version = match db
            .get_pinned_default(MIGRATION_VERSION_KEY)
            .expect("get the version of database")
        {
            Some(version_bytes) => {
                String::from_utf8(version_bytes.to_vec()).expect("version bytes to utf8")
            }
            None => {
                // if version is none, but db is not empty
                // patch 220464f
                return self.is_non_empty_rdb(db);
            }
        };

        self.migrations
            .values()
            .skip_while(|m| m.version() <= db_version.as_str())
            .any(|m| m.expensive())
    }

    pub fn run_in_background(&self, db: &ReadOnlyDB) -> bool {
        let db_version = match db
            .get_pinned_default(MIGRATION_VERSION_KEY)
            .expect("get the version of database")
        {
            Some(version_bytes) => {
                String::from_utf8(version_bytes.to_vec()).expect("version bytes to utf8")
            }
            None => {
                // if version is none, but db is not empty
                // patch 220464f
                return self.is_non_empty_rdb(db);
            }
        };

        self.migrations
            .values()
            .skip_while(|m| m.version() <= db_version.as_str())
            .all(|m| m.run_in_background())
    }

    fn is_non_empty_rdb(&self, db: &ReadOnlyDB) -> bool {
        if let Ok(v) = db.get_pinned(COLUMN_META, META_TIP_HEADER_KEY) {
            if v.is_some() {
                return true;
            }
        }
        false
    }

    fn is_non_empty_db(&self, db: &RocksDB) -> bool {
        if let Ok(v) = db.get_pinned(COLUMN_META, META_TIP_HEADER_KEY) {
            if v.is_some() {
                return true;
            }
        }
        false
    }

    fn run_migrate(&self, mut db: RocksDB, v: &str) -> Result<RocksDB, Error> {
        let mpb = Arc::new(MultiProgress::new());
        let migrations: BTreeMap<_, _> = self
            .migrations
            .iter()
            .filter(|(mv, _)| mv.as_str() > v)
            .collect();
        let migrations_count = migrations.len();
        for (idx, (_, m)) in migrations.iter().enumerate() {
            let mpbc = Arc::clone(&mpb);
            let pb = move |count: u64| -> ProgressBar {
                let pb = mpbc.add(ProgressBar::new(count));
                pb.set_draw_target(ProgressDrawTarget::term(Term::stdout(), None));
                pb.set_prefix(format!("[{}/{}]", idx + 1, migrations_count));
                pb
            };
            db = m.migrate(db, Arc::new(pb))?;
            db.put_default(MIGRATION_VERSION_KEY, m.version())
                .map_err(|err| internal_error(format!("failed to migrate the database: {err}")))?;
        }
        mpb.join_and_clear().expect("MultiProgress join");
        Ok(db)
    }

    fn run_migrate_async(&self, db: RocksDB, v: &str) {
        let migrations: Vec<(String, Arc<dyn Migration>)> = self
            .migrations
            .iter()
            .filter(|(mv, _)| mv.as_str() > v)
            .map(|(mv, m)| (mv.to_string(), Arc::clone(m)))
            .collect::<Vec<_>>();

        let tasks = Arc::new(Mutex::new(migrations));
        let worker = MigrationWorker::new(tasks, db.clone());
        worker.start();
    }

    fn get_migration_version(&self, db: &RocksDB) -> Result<Option<String>, Error> {
        let raw = db
            .get_pinned_default(MIGRATION_VERSION_KEY)
            .map_err(|err| {
                internal_error(format!("failed to get the version of database: {err}"))
            })?;

        Ok(raw.map(|version_bytes| {
            String::from_utf8(version_bytes.to_vec()).expect("version bytes to utf8")
        }))
    }

    /// Initial db version
    pub fn init_db_version(&self, db: &RocksDB) -> Result<(), Error> {
        let db_version = self.get_migration_version(db)?;
        if db_version.is_none() {
            if let Some(m) = self.migrations.values().last() {
                info!("Init database version {}", m.version());
                db.put_default(MIGRATION_VERSION_KEY, m.version())
                    .map_err(|err| {
                        internal_error(format!("failed to migrate the database: {err}"))
                    })?;
            }
        }
        Ok(())
    }

    /// TODO(doc): @quake
    pub fn migrate(&self, db: RocksDB) -> Result<RocksDB, Error> {
        let db_version = self.get_migration_version(&db)?;
        match db_version {
            Some(ref v) => {
                info!("Current database version {}", v);
                if let Some(m) = self.migrations.values().last() {
                    if m.version() < v.as_str() {
                        error!(
                            "Database downgrade detected. \
                            The database schema version is newer than client schema version,\
                            please upgrade to the newer version"
                        );
                        return Err(internal_error(
                            "Database downgrade is not supported".to_string(),
                        ));
                    }
                }

                let db = self.run_migrate(db, v.as_str())?;
                Ok(db)
            }
            None => {
                // if version is none, but db is not empty
                // patch 220464f
                if self.is_non_empty_db(&db) {
                    return self.patch_220464f(db);
                }
                Ok(db)
            }
        }
    }

    /// TODO(doc): @quake
    pub fn migrate_async(&self, db: RocksDB) -> Result<RocksDB, Error> {
        let db_version = self.get_migration_version(&db)?;
        match db_version {
            Some(ref v) => {
                info!("Current database version {}", v);
                if let Some(m) = self.migrations.values().last() {
                    if m.version() < v.as_str() {
                        error!(
                            "Database downgrade detected. \
                            The database schema version is newer than client schema version,\
                            please upgrade to the newer version"
                        );
                        return Err(internal_error(
                            "Database downgrade is not supported".to_string(),
                        ));
                    }
                }
                self.run_migrate_async(db.clone(), v.as_str());
                Ok(db)
            }
            None => {
                // if version is none, but db is not empty
                // patch 220464f
                if self.is_non_empty_db(&db) {
                    return self.patch_220464f(db);
                }
                Ok(db)
            }
        }
    }

    fn patch_220464f(&self, db: RocksDB) -> Result<RocksDB, Error> {
        const V: &str = "20210609195048"; // AddExtraDataHash - 1
        self.run_migrate(db, V)
    }
}

/// TODO(doc): @quake
pub trait Migration: Send + Sync {
    /// TODO(doc): @quake
    fn migrate(
        &self,
        _db: RocksDB,
        _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error>;

    /// returns migration version, use `date +'%Y%m%d%H%M%S'` timestamp format
    fn version(&self) -> &str;

    /// Will cost a lot of time to perform this migration operation.
    ///
    /// Override this function for `Migrations` which could be executed very fast.
    fn expensive(&self) -> bool {
        true
    }

    fn run_in_background(&self) -> bool {
        false
    }
}

/// TODO(doc): @quake
pub struct DefaultMigration {
    version: String,
}

impl DefaultMigration {
    /// TODO(doc): @quake
    pub fn new(version: &str) -> Self {
        Self {
            version: version.to_string(),
        }
    }
}

impl Migration for DefaultMigration {
    fn migrate(
        &self,
        db: RocksDB,
        _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error> {
        Ok(db)
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn expensive(&self) -> bool {
        false
    }
}
