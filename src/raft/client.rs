use raft::{SyncClient, ClientClusterInfo, RaftMsg, RaftStateMachine, LogEntry, ClientQryResponse};
use raft::state_machine::OpType;
use std::collections::{HashMap, BTreeMap, HashSet};
use std::iter::FromIterator;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::cell::RefCell;
use bifrost_plugins::hash_str;
use rand;

const ORDERING: Ordering = Ordering::Relaxed;

struct QryMeta {
    last_log_id: AtomicU64,
    last_log_term: AtomicU64,
    pos: AtomicU64
}

struct Members {
    clients: BTreeMap<u64, Mutex<SyncClient>>,
    id_map: HashMap<u64, String>,
}

pub struct RaftClient {
    qry_meta: QryMeta,
    members: Members,
    term: u64, // last member list update term
}

impl RaftClient {
    pub fn new(servers: Vec<String>) -> Option<RaftClient> {
        let mut client = RaftClient {
            qry_meta: QryMeta {
                last_log_id: AtomicU64::new(0),
                last_log_term: AtomicU64::new(0),
                pos: AtomicU64::new(rand::random::<u64>())
            },
            members: Members {
                clients: BTreeMap::new(),
                id_map: HashMap::new()
            },
            term: 0,
        };
        match client.update_info(&HashSet::from_iter(servers)) {
            Ok(_) => Some(client),
            Err(_) => None
        }
    }

    fn update_info(&mut self, addrs: &HashSet<String>) -> Result<(), ()> {
        let info: ClientClusterInfo;
        let mut servers = None;
        for server_addr in addrs {
            let id = hash_str(server_addr.clone());
            let mut client = self.members.clients.entry(id).or_insert_with(|| {
                Mutex::new(SyncClient::new(server_addr))
            });
            if let Some(Ok(info)) = client.lock().unwrap().c_server_cluster_info() {
                servers = Some(info);
                break;
            }
        }
        match servers {
            Some(servers) => {
                let remote_members = servers.members;
                let mut remote_ids = HashSet::with_capacity(remote_members.len());
                self.members.id_map.clear();
                for (id, addr) in remote_members {
                    self.members.id_map.insert(id, addr);
                    remote_ids.insert(id);
                }
                let mut connected_ids = HashSet::with_capacity(self.members.clients.len());
                for id in self.members.clients.keys() {connected_ids.insert(*id);}
                let ids_to_remove = connected_ids.difference(&remote_ids);
                for id in ids_to_remove {self.members.clients.remove(id);}
                for id in remote_ids.difference(&connected_ids) {
                    let addr = self.members.id_map.get(id).unwrap();
                    self.members.clients.entry(id.clone()).or_insert_with(|| {
                        Mutex::new(SyncClient::new(addr))
                    });
                }
                Ok(())
            },
            None => Err(()),
        }
    }

    pub fn execute<R>(&self, sm: RaftStateMachine, msg: &RaftMsg<R>) -> Option<R> {
        let (fn_id, op, req_data) = msg.encode();
        let sm_id = sm.id;
        let response = match op {
            OpType::QUERY => {

            },
            OpType::COMMAND => {

            },
        };
        None
    }

    fn query(&self, sm_id: u64, fn_id: u64, data: &Vec<u8>) -> Option<Vec<u8>> {
        let pos = self.qry_meta.pos.fetch_add(1, ORDERING);
        let clients_count = self.members.clients.len();
        let res = {
            let mut client = self.members.clients.values()
                .nth(pos as usize % clients_count)
                .unwrap().lock().unwrap();
            client.c_query(LogEntry {
                id: self.qry_meta.last_log_id.load(ORDERING),
                term: self.qry_meta.last_log_term.load(ORDERING),
                sm_id: sm_id,
                fn_id: fn_id,
                data: data.clone()
            })
        };
        match res {
            Some(Ok(res)) => {
                match res {
                    ClientQryResponse::LeftBehind => {
                        self.query(sm_id, fn_id, data)
                    },
                    ClientQryResponse::Success{
                        data: data,
                        last_log_term: last_log_term,
                        last_log_id: last_log_id
                    } => {
                        swap_when_larger(&self.qry_meta.last_log_id, last_log_id);
                        swap_when_larger(&self.qry_meta.last_log_term, last_log_term);
                        Some(data)
                    },
                }
            },
            _ => None
        }
    }
}

fn swap_when_larger(atomic: &AtomicU64, value: u64) {
    let mut orig_num = atomic.load(ORDERING);
    loop {
        if orig_num >= value {
            return;
        }
        let actual = atomic.compare_and_swap(orig_num, value, ORDERING);
        if actual == orig_num {
            return;
        } else {
            orig_num = actual;
        }
    }
}