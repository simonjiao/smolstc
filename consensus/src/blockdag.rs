use consensus_types::{
    blockhash::{BlockHashes, KType, ORIGIN},
    header::{ConsensusHeader, Header},
};
use database::consensus::{
    DbGhostdagStore, DbHeadersStore, DbReachabilityStore, DbRelationsStore, GhostdagStore,
    HeaderStore, ReachabilityStoreReader, RelationsStore, RelationsStoreReader,
};
use database::prelude::FlexiDagStorage;
use ghostdag::protocol::GhostdagManager;
use parking_lot::RwLock;
use reachability::{inquirer, reachability_service::MTReachabilityService};
use starcoin_crypto::HashValue as Hash;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

pub type DbGhostdagManager = GhostdagManager<
    DbGhostdagStore,
    DbRelationsStore,
    MTReachabilityService<DbReachabilityStore>,
    DbHeadersStore,
>;
pub struct BlockDAG {
    genesis: Header,
    ghostdag_manager: DbGhostdagManager,
    relations_store: DbRelationsStore,
    reachability_store: DbReachabilityStore,
    ghostdag_store: DbGhostdagStore,
    header_store: DbHeadersStore,
    /// orphan blocks, parent hash -> orphan block
    missing_blocks: HashMap<Hash, HashSet<Header>>,
}

impl BlockDAG {
    pub fn new(genesis: Header, k: KType, db: FlexiDagStorage) -> Self {
        let ghostdag_store = db.ghost_dag_store.clone();
        let header_store = db.header_store.clone();
        let relations_store = db.relations_store.clone();
        let mut reachability_store = db.reachability_store;
        inquirer::init(&mut reachability_store).unwrap();
        let reachability_service =
            MTReachabilityService::new(Arc::new(RwLock::new(reachability_store.clone())));
        let ghostdag_manager = DbGhostdagManager::new(
            genesis.hash(),
            k,
            ghostdag_store.clone(),
            relations_store.clone(),
            header_store.clone(),
            reachability_service,
        );

        let mut dag = Self {
            genesis,
            ghostdag_manager,
            relations_store,
            reachability_store,
            ghostdag_store,
            header_store,
            missing_blocks: HashMap::new(),
        };
        dag.init_with_genesis();
        dag
    }

    pub fn init_with_genesis(&mut self) {
        if self.relations_store.has(Hash::new(ORIGIN)).unwrap() {
            return;
        }
        self.relations_store
            .insert(Hash::new(ORIGIN), BlockHashes::new(vec![]))
            .unwrap();
        self.commit_header(&self.genesis.clone())
    }

    pub fn commit_header(&mut self, header: &Header) {
        // Generate ghostdag data

        let parents_hash = header.parents_hash();
        let ghostdag_data = if header.hash() != self.genesis.hash() {
            self.ghostdag_manager.ghostdag(parents_hash)
        } else {
            self.ghostdag_manager.genesis_ghostdag_data()
        };
        // Store ghostdata
        self.ghostdag_store
            .insert(header.hash(), Arc::new(ghostdag_data.clone()))
            .unwrap();

        // Update reachability store
        let mut reachability_store = self.reachability_store.clone();
        let mut merge_set = ghostdag_data
            .unordered_mergeset_without_selected_parent()
            .filter(|hash| self.reachability_store.has(*hash).unwrap());

        inquirer::add_block(
            &mut reachability_store,
            header.hash(),
            ghostdag_data.selected_parent,
            &mut merge_set,
        )
        .unwrap();

        // store relations
        self.relations_store
            .insert(header.hash(), BlockHashes::new(parents_hash.to_vec()))
            .unwrap();
        // Store header store
        self.header_store
            .insert(header.hash(), Arc::new(header.to_owned()), 0)
            .unwrap();
    }
    fn is_in_dag(&self, hash: Hash) -> anyhow::Result<bool> {
        return Ok(true);
    }
    pub fn verify_header(&self, header: &Header) -> anyhow::Result<()> {
        //TODO: implemented it
        Ok(())
    }

    pub fn connect_block(&mut self, header: &Header) -> anyhow::Result<()> {
        let _ = self.verify_header(header)?;
        let is_orphan_block = self.update_orphans(header)?;
        if is_orphan_block {
            return Ok(());
        }
        self.commit_header(header);
        self.check_missing_block(header)?;
        Ok(())
    }

    pub fn check_missing_block(&mut self, header: &Header) -> anyhow::Result<()> {
        if let Some(orphans) = self.missing_blocks.remove(&header.hash()) {
            for orphan in orphans.iter() {
                let is_orphan = self.is_orphan(&orphan)?;
                if !is_orphan {
                    self.commit_header(header);
                }
            }
        }
        Ok(())
    }
    fn is_orphan(&self, header: &Header) -> anyhow::Result<bool> {
        for parent in header.parents_hash() {
            if !self.is_in_dag(parent.to_owned())? {
                return Ok(false);
            }
        }
        return Ok(true);
    }

    fn update_orphans(&mut self, block_header: &Header) -> anyhow::Result<bool> {
        let mut is_orphan = false;
        for parent in block_header.parents_hash() {
            if self.is_in_dag(parent.to_owned())? {
                continue;
            }
            if !self
                .missing_blocks
                .entry(parent.to_owned())
                .or_insert_with(HashSet::new)
                .insert(block_header.to_owned())
            {
                return Err(anyhow::anyhow!("Block already processed as a orphan"));
            }
            is_orphan = true;
        }
        Ok(is_orphan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use database::prelude::{FlexiDagStorage, FlexiDagStorageConfig};
    use starcoin_types::block::BlockHeader;
    use std::{env, fs};
    #[test]
    fn base_test() {
        let genesis = Header::new(BlockHeader::random(), vec![Hash::new(ORIGIN)]);
        let genesis_hash = genesis.hash();
        let k = 16;
        let db_path = env::temp_dir().join("smolstc");
        println!("db path:{}", db_path.to_string_lossy());
        if db_path
            .as_path()
            .try_exists()
            .unwrap_or_else(|_| panic!("Failed to check {db_path:?}"))
        {
            fs::remove_dir_all(db_path.as_path()).expect("Failed to delete temporary directory");
        }
        let config = FlexiDagStorageConfig::create_with_params(1, 0, 1024);
        let db = FlexiDagStorage::create_from_path(db_path, config)
            .expect("Failed to create flexidag storage");
        let mut dag = BlockDAG::new(genesis, k, db);

        let block = Header::new(BlockHeader::random(), vec![genesis_hash]);
        dag.commit_header(&block);
    }
}
