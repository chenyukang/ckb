use super::rich_indexer::RichIndexerConfig;

use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Indexer config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexerConfig {
    /// The index store path, default `data_dir / indexer / store`
    #[serde(default)]
    pub store: PathBuf,
    /// The secondary_db path, default `data_dir / indexer / secondary_path`
    #[serde(default)]
    pub secondary_path: PathBuf,
    /// The poll interval by secs
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
    /// Whether to index the pending txs in the ckb tx-pool
    #[serde(default)]
    pub index_tx_pool: bool,
    /// Customize block filter
    #[serde(default)]
    pub block_filter: Option<String>,
    /// Customize cell filter
    #[serde(default)]
    pub cell_filter: Option<String>,
    /// Maximum number of concurrent db background jobs (compactions and flushes)
    #[serde(default)]
    pub db_background_jobs: Option<NonZeroUsize>,
    /// Maximal db info log files to be kept.
    #[serde(default)]
    pub db_keep_log_file_num: Option<NonZeroUsize>,
    /// The init tip block hash
    #[serde(default)]
    pub init_tip_hash: Option<H256>,
    /// Rich indexer config options
    #[serde(default)]
    pub rich_indexer: RichIndexerConfig,
    /// Max iterator next limit count
    #[serde(default = "default_iterator_next_limit")]
    pub iterator_next_limit: usize,
}

const fn default_poll_interval() -> u64 {
    2
}

const fn default_iterator_next_limit() -> usize {
    100_000
}

impl Default for IndexerConfig {
    fn default() -> Self {
        IndexerConfig {
            poll_interval: default_poll_interval(),
            index_tx_pool: false,
            store: PathBuf::new(),
            secondary_path: PathBuf::new(),
            block_filter: None,
            cell_filter: None,
            db_background_jobs: None,
            db_keep_log_file_num: None,
            init_tip_hash: None,
            rich_indexer: RichIndexerConfig::default(),
            iterator_next_limit: default_iterator_next_limit(),
        }
    }
}

impl IndexerConfig {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.store` is not set, set it to `data_dir / indexer / store`.
    ///
    /// If `self.secondary_path` is not set, set it to `data_dir / indexer / secondary_path`.
    ///
    /// If `self.rich_indexer` is `Sqlite`, and `self.rich_indexer.sqlite.store` is not set,
    /// set it to `data_dir / indexer / sqlite / sqlite.db`.
    ///
    /// If any of the above paths is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, indexer_dir: P) {
        _adjust(root_dir, indexer_dir.as_ref(), &mut self.store, "store");
        _adjust(
            root_dir,
            indexer_dir.as_ref(),
            &mut self.secondary_path,
            "secondary_path",
        );
        _adjust(
            root_dir,
            indexer_dir.as_ref(),
            &mut self.rich_indexer.store,
            "sqlite/sqlite.db",
        );
    }
}

fn _adjust(root_dir: &Path, indexer_dir: &Path, target: &mut PathBuf, sub: &str) {
    if target.to_str().is_none() || target.to_str() == Some("") {
        *target = indexer_dir.to_path_buf().join(sub);
    } else if target.is_relative() {
        *target = root_dir.to_path_buf().join(&target)
    }
}

/// Indexer sync config options.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexerSyncConfig {
    /// The secondary_db path, default `data_dir / indexer / secondary_path`
    #[serde(default)]
    pub secondary_path: PathBuf,
    /// The poll interval by secs
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
    /// Whether to index the pending txs in the ckb tx-pool
    pub index_tx_pool: bool,
    /// Maximal db info log files to be kept.
    #[serde(default)]
    pub db_keep_log_file_num: Option<NonZeroUsize>,
}

impl From<&IndexerConfig> for IndexerSyncConfig {
    fn from(config: &IndexerConfig) -> IndexerSyncConfig {
        IndexerSyncConfig {
            secondary_path: config.secondary_path.clone(),
            poll_interval: config.poll_interval,
            index_tx_pool: config.index_tx_pool,
            db_keep_log_file_num: config.db_keep_log_file_num,
        }
    }
}
