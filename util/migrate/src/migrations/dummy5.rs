use ckb_app_config::StoreConfig;
use ckb_db_migration::{Migration, ProgressBar};
use ckb_store::ChainDB;
use std::thread;
use std::time::Duration;

const VERSION: &str = "20231101000005";
pub struct Dummy5;

impl Migration for Dummy5 {
    fn run_in_background(&self) -> bool {
        true
    }

    fn migrate(
        &self,
        db: ckb_db::RocksDB,
        _pb: std::sync::Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<ckb_db::RocksDB, ckb_error::Error> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let mut time = 10;
        while time >= 0 {
            eprintln!("running now dummy5 ....");
            thread::sleep(Duration::from_secs(1));
            time -= 1;
        }

        Ok(chain_db.into_inner())
    }
    fn version(&self) -> &str {
        VERSION
    }
}
