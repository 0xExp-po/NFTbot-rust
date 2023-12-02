use bincode::serialize;
use indexmap::IndexMap;
use lazy_static::lazy_static;
use log::*;
use rand::{thread_rng, Rng};
use solana_client::{
    connection_cache::Connection,
    pubsub_client::{PubsubClient, PubsubClientSubscription},
    quic_client::QuicTpuConnection,
    rpc_client::RpcClient,
    rpc_response::SlotUpdate,
    tpu_client::{TpuClientConfig, TpuSenderError, MAX_FANOUT_SLOTS},
    tpu_connection::{ClientStats, TpuConnection},
    udp_client::UdpTpuConnection,
};
use solana_sdk::{
    clock::Slot,
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    transaction::Transaction,
    transport::{Result as TransportResult, TransportError},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{SocketAddr, UdpSocket},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

lazy_static! {
    static ref CONNECTION_MAP: RwLock<ConnectionMap> = RwLock::new(ConnectionMap::new());
}

static MAX_CONNECTIONS: usize = 1024;

struct ConnectionMap {
    map: IndexMap<SocketAddr, Connection>,
    use_quic: bool,
}

impl ConnectionMap {
    pub fn new() -> Self {
        Self {
            map: IndexMap::with_capacity(MAX_CONNECTIONS),
            use_quic: false,
        }
    }
}

type TpuResult<T> = std::result::Result<T, TpuSenderError>;

pub struct TpuClient {
    _deprecated: UdpSocket, // TpuClient now uses the connection_cache to choose a send_socket
    fanout_slots: u64,
    leader_tpu_service: LeaderTpuService,
    exit: Arc<AtomicBool>,
}

impl TpuClient {
    /// Serialize and send transaction to the current and upcoming leader TPUs according to fanout
    /// size
    // pub fn send_transaction(&self, transaction: &Transaction) -> bool {
    //     let wire_transaction = serialize(transaction).expect("serialization should succeed");
    //     self.send_wire_transaction(wire_transaction)
    // }

    /// Send a wire transaction to the current and upcoming leader TPUs according to fanout size
    // pub fn send_wire_transaction(&self, wire_transaction: Vec<u8>) -> bool {
    //     self.try_send_wire_transaction(wire_transaction).is_ok()
    // }

    pub fn estimated_current_slot(&self) -> Slot {
        self.leader_tpu_service
            .recent_slots
            .estimated_current_slot()
    }

    /// Serialize and send transaction to the current and upcoming leader TPUs according to fanout
    /// size
    /// Returns the last error if all sends fail
    pub fn try_send_transaction(&self, transaction: &Transaction) -> TransportResult<()> {
        let wire_transaction = serialize(transaction).expect("serialization should succeed");
        self.try_send_wire_transaction(wire_transaction)
    }

    /// Send a wire transaction to the current and upcoming leader TPUs according to fanout size
    /// Returns the last error if all sends fail
    fn try_send_wire_transaction(&self, wire_transaction: Vec<u8>) -> TransportResult<()> {
        let mut last_error: Option<TransportError> = None;
        let mut some_success = false;

        for tpu_address in self
            .leader_tpu_service
            .leader_tpu_sockets(self.fanout_slots)
        {
            let result = send_wire_transaction_async(wire_transaction.clone(), &tpu_address);
            if let Err(err) = result {
                last_error = Some(err);
            } else {
                some_success = true;
            }
        }
        if !some_success {
            Err(if let Some(err) = last_error {
                err
            } else {
                std::io::Error::new(std::io::ErrorKind::Other, "No sends attempted").into()
            })
        } else {
            Ok(())
        }
    }

    /// Create a new client that disconnects when dropped
    pub fn new(
        rpc_client: Arc<RpcClient>,
        websocket_url: &str,
        config: TpuClientConfig,
    ) -> TpuResult<Self> {
        let exit = Arc::new(AtomicBool::new(false));
        let leader_tpu_service =
            LeaderTpuService::new(rpc_client.clone(), websocket_url, exit.clone())?;

        Ok(Self {
            _deprecated: UdpSocket::bind("0.0.0.0:0").unwrap(),
            fanout_slots: config.fanout_slots.min(MAX_FANOUT_SLOTS).max(1),
            leader_tpu_service,
            exit,
        })
    }
}

impl Drop for TpuClient {
    fn drop(&mut self) {
        self.exit.store(true, Ordering::Relaxed);
        self.leader_tpu_service.join();
    }
}

fn send_wire_transaction_async(packets: Vec<u8>, addr: &SocketAddr) -> Result<(), TransportError> {
    let conn = get_or_add_connection(addr);
    let client_stats = Arc::new(ClientStats::default());
    let r = match conn {
        Connection::Udp(conn) => conn.send_wire_transaction_async(packets, client_stats.clone()),
        Connection::Quic(conn) => conn.send_wire_transaction_async(packets, client_stats.clone()),
    };
    r
}

fn get_or_add_connection(addr: &SocketAddr) -> Connection {
    let map = (*CONNECTION_MAP).read().unwrap();

    match map.map.get(addr) {
        Some(connection) => connection.clone(),
        None => {
            // Upgrade to write access by dropping read lock and acquire write lock
            drop(map);
            let mut map = (*CONNECTION_MAP).write().unwrap();

            // Read again, as it is possible that between read lock dropped and the write lock acquired
            // another thread could have setup the connection.
            match map.map.get(addr) {
                Some(connection) => connection.clone(),
                None => {
                    let connection = if map.use_quic {
                        Connection::Quic(Arc::new(QuicTpuConnection::new(*addr)))
                    } else {
                        Connection::Udp(Arc::new(UdpTpuConnection::new(*addr)))
                    };

                    while map.map.len() >= MAX_CONNECTIONS {
                        let mut rng = thread_rng();
                        let n = rng.gen_range(0..MAX_CONNECTIONS);
                        map.map.swap_remove_index(n);
                    }

                    map.map.insert(*addr, connection.clone());
                    connection
                }
            }
        }
    }
}

struct LeaderTpuCache {
    first_slot: Slot,
    leaders: Vec<Pubkey>,
    leader_tpu_map: HashMap<Pubkey, SocketAddr>,
    slots_in_epoch: Slot,
    last_epoch_info_slot: Slot,
}

impl LeaderTpuCache {
    fn new(rpc_client: &RpcClient, first_slot: Slot) -> TpuResult<Self> {
        let slots_in_epoch = rpc_client.get_epoch_info()?.slots_in_epoch;
        let leaders = Self::fetch_slot_leaders(rpc_client, first_slot, slots_in_epoch)?;
        let leader_tpu_map = Self::fetch_cluster_tpu_sockets(rpc_client)?;
        Ok(Self {
            first_slot,
            leaders,
            leader_tpu_map,
            slots_in_epoch,
            last_epoch_info_slot: first_slot,
        })
    }

    // Last slot that has a cached leader pubkey
    fn last_slot(&self) -> Slot {
        self.first_slot + self.leaders.len().saturating_sub(1) as u64
    }

    // Get the TPU sockets for the current leader and upcoming leaders according to fanout size
    fn get_leader_sockets(&self, current_slot: Slot, fanout_slots: u64) -> Vec<SocketAddr> {
        let mut leader_set = HashSet::new();
        let mut leader_sockets = Vec::new();
        for leader_slot in current_slot..current_slot + fanout_slots {
            if let Some(leader) = self.get_slot_leader(leader_slot) {
                if let Some(tpu_socket) = self.leader_tpu_map.get(leader) {
                    if leader_set.insert(*leader) {
                        leader_sockets.push(*tpu_socket);
                    }
                } else {
                    // The leader is probably delinquent
                    trace!("TPU not available for leader {}", leader);
                }
            } else {
                // Overran the local leader schedule cache
                warn!(
                    "Leader not known for slot {}; cache holds slots [{},{}]",
                    leader_slot,
                    self.first_slot,
                    self.last_slot()
                );
            }
        }
        leader_sockets
    }

    fn get_slot_leader(&self, slot: Slot) -> Option<&Pubkey> {
        if slot >= self.first_slot {
            let index = slot - self.first_slot;
            self.leaders.get(index as usize)
        } else {
            None
        }
    }

    fn fetch_cluster_tpu_sockets(rpc_client: &RpcClient) -> TpuResult<HashMap<Pubkey, SocketAddr>> {
        let cluster_contact_info = rpc_client.get_cluster_nodes()?;
        Ok(cluster_contact_info
            .into_iter()
            .filter_map(|contact_info| {
                Some((
                    Pubkey::from_str(&contact_info.pubkey).ok()?,
                    contact_info.tpu?,
                ))
            })
            .collect())
    }

    fn fetch_slot_leaders(
        rpc_client: &RpcClient,
        start_slot: Slot,
        slots_in_epoch: Slot,
    ) -> TpuResult<Vec<Pubkey>> {
        let fanout = (2 * MAX_FANOUT_SLOTS).min(slots_in_epoch);
        Ok(rpc_client.get_slot_leaders(start_slot, fanout)?)
    }
}

// 48 chosen because it's unlikely that 12 leaders in a row will miss their slots
const MAX_SLOT_SKIP_DISTANCE: u64 = 48;

#[derive(Clone, Debug)]
struct RecentLeaderSlots(Arc<RwLock<VecDeque<Slot>>>);
impl RecentLeaderSlots {
    fn new(current_slot: Slot) -> Self {
        let mut recent_slots = VecDeque::new();
        recent_slots.push_back(current_slot);
        Self(Arc::new(RwLock::new(recent_slots)))
    }

    fn record_slot(&self, current_slot: Slot) {
        let mut recent_slots = self.0.write().unwrap();
        recent_slots.push_back(current_slot);
        // 12 recent slots should be large enough to avoid a misbehaving
        // validator from affecting the median recent slot
        while recent_slots.len() > 12 {
            recent_slots.pop_front();
        }
    }

    // Estimate the current slot from recent slot notifications.
    fn estimated_current_slot(&self) -> Slot {
        let mut recent_slots: Vec<Slot> = self.0.read().unwrap().iter().cloned().collect();
        assert!(!recent_slots.is_empty());
        recent_slots.sort_unstable();

        // Validators can broadcast invalid blocks that are far in the future
        // so check if the current slot is in line with the recent progression.
        let max_index = recent_slots.len() - 1;
        let median_index = max_index / 2;
        let median_recent_slot = recent_slots[median_index];
        let expected_current_slot = median_recent_slot + (max_index - median_index) as u64;
        let max_reasonable_current_slot = expected_current_slot + MAX_SLOT_SKIP_DISTANCE;

        // Return the highest slot that doesn't exceed what we believe is a
        // reasonable slot.
        recent_slots
            .into_iter()
            .rev()
            .find(|slot| *slot <= max_reasonable_current_slot)
            .unwrap()
    }
}

/// Service that tracks upcoming leaders and maintains an up-to-date mapping
/// of leader id to TPU socket address.
struct LeaderTpuService {
    recent_slots: RecentLeaderSlots,
    leader_tpu_cache: Arc<RwLock<LeaderTpuCache>>,
    subscription: Option<PubsubClientSubscription<SlotUpdate>>,
    t_leader_tpu_service: Option<JoinHandle<()>>,
}

impl LeaderTpuService {
    fn new(
        rpc_client: Arc<RpcClient>,
        websocket_url: &str,
        exit: Arc<AtomicBool>,
    ) -> TpuResult<Self> {
        let start_slot = rpc_client.get_slot_with_commitment(CommitmentConfig::processed())?;

        let recent_slots = RecentLeaderSlots::new(start_slot);
        let leader_tpu_cache = Arc::new(RwLock::new(LeaderTpuCache::new(&rpc_client, start_slot)?));

        let subscription = if !websocket_url.is_empty() {
            let recent_slots = recent_slots.clone();
            Some(PubsubClient::slot_updates_subscribe(
                websocket_url,
                move |update| {
                    let current_slot = match update {
                        // This update indicates that a full slot was received by the connected
                        // node so we can stop sending transactions to the leader for that slot
                        SlotUpdate::Completed { slot, .. } => slot.saturating_add(1),
                        // This update indicates that we have just received the first shred from
                        // the leader for this slot and they are probably still accepting transactions.
                        SlotUpdate::FirstShredReceived { slot, .. } => slot,
                        _ => return,
                    };
                    recent_slots.record_slot(current_slot);
                },
            )?)
        } else {
            None
        };

        let t_leader_tpu_service = Some({
            let recent_slots = recent_slots.clone();
            let leader_tpu_cache = leader_tpu_cache.clone();
            std::thread::Builder::new()
                .name("ldr-tpu-srv".to_string())
                .spawn(move || Self::run(rpc_client, recent_slots, leader_tpu_cache, exit))
                .unwrap()
        });

        Ok(LeaderTpuService {
            recent_slots,
            leader_tpu_cache,
            subscription,
            t_leader_tpu_service,
        })
    }

    fn join(&mut self) {
        if let Some(mut subscription) = self.subscription.take() {
            let _ = subscription.send_unsubscribe();
            let _ = subscription.shutdown();
        }
        if let Some(t_handle) = self.t_leader_tpu_service.take() {
            t_handle.join().unwrap();
        }
    }

    fn leader_tpu_sockets(&self, fanout_slots: u64) -> Vec<SocketAddr> {
        let current_slot = self.recent_slots.estimated_current_slot();
        self.leader_tpu_cache
            .read()
            .unwrap()
            .get_leader_sockets(current_slot, fanout_slots)
    }

    fn run(
        rpc_client: Arc<RpcClient>,
        recent_slots: RecentLeaderSlots,
        leader_tpu_cache: Arc<RwLock<LeaderTpuCache>>,
        exit: Arc<AtomicBool>,
    ) {
        let mut last_cluster_refresh = Instant::now();
        let mut sleep_ms = 1000;
        loop {
            if exit.load(Ordering::Relaxed) {
                break;
            }

            // Sleep a few slots before checking if leader cache needs to be refreshed again
            std::thread::sleep(Duration::from_millis(sleep_ms));
            sleep_ms = 1000;

            // Refresh cluster TPU ports every 5min in case validators restart with new port configuration
            // or new validators come online
            if last_cluster_refresh.elapsed() > Duration::from_secs(5 * 60) {
                match LeaderTpuCache::fetch_cluster_tpu_sockets(&rpc_client) {
                    Ok(leader_tpu_map) => {
                        leader_tpu_cache.write().unwrap().leader_tpu_map = leader_tpu_map;
                        last_cluster_refresh = Instant::now();
                    }
                    Err(err) => {
                        warn!("Failed to fetch cluster tpu sockets: {}", err);
                        sleep_ms = 100;
                    }
                }
            }

            let estimated_current_slot = recent_slots.estimated_current_slot();
            let (last_slot, last_epoch_info_slot, mut slots_in_epoch) = {
                let leader_tpu_cache = leader_tpu_cache.read().unwrap();
                (
                    leader_tpu_cache.last_slot(),
                    leader_tpu_cache.last_epoch_info_slot,
                    leader_tpu_cache.slots_in_epoch,
                )
            };
            if estimated_current_slot >= last_epoch_info_slot.saturating_sub(slots_in_epoch) {
                if let Ok(epoch_info) = rpc_client.get_epoch_info() {
                    slots_in_epoch = epoch_info.slots_in_epoch;
                    let mut leader_tpu_cache = leader_tpu_cache.write().unwrap();
                    leader_tpu_cache.slots_in_epoch = slots_in_epoch;
                    leader_tpu_cache.last_epoch_info_slot = estimated_current_slot;
                }
            }
            if estimated_current_slot >= last_slot.saturating_sub(MAX_FANOUT_SLOTS) {
                match LeaderTpuCache::fetch_slot_leaders(
                    &rpc_client,
                    estimated_current_slot,
                    slots_in_epoch,
                ) {
                    Ok(slot_leaders) => {
                        let mut leader_tpu_cache = leader_tpu_cache.write().unwrap();
                        leader_tpu_cache.first_slot = estimated_current_slot;
                        leader_tpu_cache.leaders = slot_leaders;
                    }
                    Err(err) => {
                        warn!(
                            "Failed to fetch slot leaders (current estimated slot: {}): {}",
                            estimated_current_slot, err
                        );
                        sleep_ms = 100;
                    }
                }
            }
        }
    }
}
