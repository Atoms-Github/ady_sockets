#![allow(dead_code)]
#![allow(unused_variables)]
#![allow(unused_imports)]
#![allow(unused_mut)]
#![allow(unused_unsafe)]
#![allow(unused_attributes)]


mod compression;
use crossbeam_channel::*;
use std::net::{SocketAddr, UdpSocket, TcpListener, TcpStream, Shutdown};
use std::collections::HashMap;
use std::{thread, io};
use std::io::{Error, ErrorKind, Write, Read};

use bimap::BiMap;
use std::sync::{Arc, RwLock};
use std::str::FromStr;
use std::time::Duration;
use serde::*;
use serde::de::DeserializeOwned;

type PlayerID = u32;

#[cfg(test)]
mod tests {
    use crate::{start_server, start_client, OutMsg};
    use std::time::Duration;
    use std::thread;
    use rand::Rng;

    #[test]
    fn test_server_a() {
        let mut server = start_server::<String>("25.49.223.181:7767".to_string());

        assert_eq!(server.recv(), OutMsg::PlayerConnected(0));
        server.send_msg(0, "anyoneThere1".to_string(), true);
        server.send_msg(0, "anyoneThere2".to_string(), true);
        server.send_msg(0, "anyoneThere3".to_string(), true);

        assert_eq!(server.recv(), OutMsg::NewFax(0, "yes1".to_string()));
        assert_eq!(server.recv(), OutMsg::NewFax(0, "yes2".to_string()));

        server.send_msg(0, "anyoneElse".to_string(), true);
        assert_eq!(server.recv(), OutMsg::PlayerConnected(1));

        assert_eq!(server.recv(), OutMsg::NewFax(1, "2hi1".to_string()));
        assert_eq!(server.recv(), OutMsg::NewFax(1, "2hi2".to_string()));
    }
    #[test]
    fn test_client_a() {
        let mut client_1 = start_client::<String>("25.49.223.181:7767".to_string());
        assert_eq!(client_1.recv(), "anyoneThere1");
        assert_eq!(client_1.recv(), "anyoneThere2");
        assert_eq!(client_1.recv(), "anyoneThere3");
        client_1.send_msg("yes1".to_string(), false);
        client_1.send_msg("yes2".to_string(), false);

        assert_eq!(client_1.recv(), "anyoneElse");

        let mut client_2 = start_client::<String>("25.49.223.181:7767".to_string());
        client_2.send_msg("2hi1".to_string(), false);
        client_2.send_msg("2hi2".to_string(), false);
        thread::sleep(Duration::from_millis(500));
    }

    #[test]
    fn test_server_b() {
        let mut big_data_server = start_server::<Vec<i32>>("25.49.223.181:7768".to_string());
        let mut data = vec![];
        let mut rand = rand::thread_rng();
        for i in 0..1_000_000{
            data.push(rand.gen::<i32>());
        }

        assert_eq!(big_data_server.recv(), OutMsg::PlayerConnected(0));
        big_data_server.send_msg(0, data, true);
        assert_eq!(big_data_server.recv(),  OutMsg::NewFax(0, vec![0]));
        thread::sleep(Duration::from_millis(1000));
    }
    #[test]
    fn test_client_b() {
        let mut client_1 = start_client::<Vec<i32>>("25.49.223.181:7768".to_string());
        let big_data = client_1.recv();
        assert_eq!(big_data.len(), 1_000_000);
        client_1.send_msg(vec![0], true);
        thread::sleep(Duration::from_millis(1000));
    }
}





enum FromNodeMsg{
    IveDied(SocketAddr),
}
enum ToNodeMsg<T>{
    Die,
    NewFax(T),
}

struct NetHubBottom<T : Send>{
    down: Receiver<DownMsg<T>>,
    up: Sender<UpMsg<T>>,
    kill_switches: HashMap<SocketAddr, Sender<ToNodeMsg<T>>>, // Rule: A thread can't be alive if it hasn't
    // Got a thing in kill switches.
}

impl<T :  'static + Send + Serialize + DeserializeOwned> NetHubBottom<T>{
    fn bind_sockets(&self, host_address : String) -> (UdpSocket, TcpListener){
        let tcp_address = host_address.parse::<SocketAddr>().unwrap();
        let mut udp_address = tcp_address.clone();
        udp_address.set_port(tcp_address.port() + 1);

        let tcp_listener = TcpListener::bind(&tcp_address).expect("Unable to bind ");
        let udp_socket = UdpSocket::bind(&udp_address).expect("Unable to bind udp hosting address.");

        log::info!("Starting hosting on tcp {} and udp on port +1", tcp_address);
        return (udp_socket, tcp_listener);
    }
    fn start(mut self, host_address : String){
        let (from_node_tx, from_node_rx) = unbounded();
        thread::spawn(move ||{
            let (mut udp,mut tcp) = self.bind_sockets(host_address);
            self.start_listen_udp(udp.try_clone().unwrap(), self.up.clone());
            let (new_tx, new_rx) = unbounded();
            thread::spawn(move ||{
                for new_stream in tcp.incoming(){
                    new_tx.send(new_stream.unwrap()).unwrap();
                }
            });

            // Listen for new peers.
            loop{
                crossbeam_channel::select! {
                    recv(new_rx) -> new_stream_maybe =>{
                        let new_stream = new_stream_maybe.unwrap();
                        let address = new_stream.peer_addr().unwrap();
                        if self.kill_switches.contains_key(&address){
                            println!("Already connected to peer {}. Ignoring connection.", address);
                            break;
                        }
                        self.up.send(UpMsg::PlayerConnected(new_stream.peer_addr().unwrap())).unwrap();
                        let killable = launch_node(new_stream, self.up.clone(), from_node_tx.clone());
                        self.kill_switches.insert(address, killable);
                    },
                    recv(self.down) -> new_downwards_maybe =>{
                        let new_downwards = new_downwards_maybe.unwrap();
                        match new_downwards{
                            DownMsg::SendFax(address, data, reliable) => {
                                if reliable{
                                    // If this fails, then a thread isn't alive rec from that port, so we don't care.
                                    if let Some(sender) = self.kill_switches.get(&address){
                                        sender.send(ToNodeMsg::NewFax(data)).unwrap();
                                    }
                                }else{
                                    udp_send(&mut udp, &data, &address);
                                }
                            }
                            DownMsg::DropPlayer(address) => {
                                // Its just a 'hint' to die. If its dying or already dead, we don't care.
                                if let Some(sender) = self.kill_switches.get(&address){
                                    sender.send(ToNodeMsg::Die).unwrap();
                                }
                            }
                        }
                    },
                    recv(from_node_rx) -> from_node_maybe =>{
                        match from_node_maybe.unwrap(){
                            FromNodeMsg::IveDied(address) => {
                                self.kill_switches.remove(&address).unwrap(); // Should always be here to unwrap.
                                self.up.send(UpMsg::PlayerDisconnected(address)).unwrap();
                            }
                        }
                    }
                };
            }
        });
    }
    fn start_listen_udp(&mut self, mut socket: UdpSocket, up: Sender<UpMsg<T>>){
        thread::Builder::new().name("StreamDeserializerUDP".to_string()).spawn(move ||{
            loop{
                let result = udp_recv(&mut socket);
                if let Some((address, fax)) = result{
                    up.send(UpMsg::NewFax(address, fax)).unwrap();
                }
            }
        }).unwrap();
    }
}
fn launch_node<T : 'static +  Send + Serialize + DeserializeOwned>
(mut socket: TcpStream, up: Sender<UpMsg<T>>, from_node: Sender<FromNodeMsg>) -> Sender<ToNodeMsg<T>>{
    let address = socket.peer_addr().unwrap();
    let mut stream_recv = socket.try_clone().unwrap();
    let (tx_kill, rx_kill) = unbounded();
    thread::spawn(move ||{
        loop{
            let data = tcp_recv(&mut stream_recv);
            match data{
                None => {
                    from_node.send(FromNodeMsg::IveDied(address)).unwrap();
                    return; // Kill thread.
                }
                Some(data) => {
                    up.send(UpMsg::NewFax(address.clone(), data)).unwrap();
                }
            }
        }
    });
    thread::spawn(move ||{
        loop{
            let node_msg = rx_kill.recv().unwrap();
            match node_msg{
                ToNodeMsg::Die => {
                    socket.shutdown(Shutdown::Both).unwrap();
                    return;
                }
                ToNodeMsg::NewFax(mut message) => {
                    tcp_write(&mut socket, &mut message);
                }
            }
        }
    });
    return tx_kill;
}

fn tcp_recv<T : Send + DeserializeOwned>(mut stream: &mut TcpStream) -> Option<T>{
    let mut message_size_buffer = vec![0; 4];
    let message_size_peek_maybe = stream.read_exact(&mut message_size_buffer);
    return match message_size_peek_maybe {
        Result::Err(error) => {
            log::warn!("Can't tcp recv. {}", error.to_string());
            None
        }
        Result::Ok(()) => {
            // Should read all 4 for size.
            let content_size = bincode::deserialize::<u32>(&message_size_buffer).unwrap() as usize;
            let mut message_buffer_compressed = vec![0; content_size];
            let content_read_size = stream.read_exact(&mut message_buffer_compressed).unwrap();
            let message_decompressed = compression::decompress(message_buffer_compressed);

            let result = bincode::deserialize::<T>(&message_decompressed);
            match result{
                Ok(msg) => {
                    Some(msg)
                }
                Err(err) => {
                    log::warn!("Can't deserialize inc tcp message.");
                    None
                }
            }
        }
    }
}


fn tcp_write<T :  'static + Send + Serialize + DeserializeOwned>(socket: &mut TcpStream, message: &T){
    let mut compressed_contents_bytes = compression::compress(bincode::serialize(message).unwrap());

    let message_size : u32 = compressed_contents_bytes.len() as u32;
    // Prepend message size.
    let mut message_wire_bytes = bincode::serialize(&message_size).unwrap();
    message_wire_bytes.append(&mut compressed_contents_bytes);
    socket.write_all(&message_wire_bytes).unwrap();
    socket.flush().unwrap();
}
fn udp_recv<T  : Send + Serialize + DeserializeOwned>(socket: &mut UdpSocket) -> Option<(SocketAddr, T)>{
    let mut message_buffer = [0; 65_535]; // Efficiency ...
    match socket.recv_from(&mut message_buffer){
        Result::Err(error) => {
            log::warn!("Failed to receive udp message from someone {:?}", error);
            None
        }
        Result::Ok((0, address)) => {
            log::warn!("Udp read 0 bytes from {}", address.to_string());
            None
        }
        Ok((message_size_bytes, address)) => {
            let result = bincode::deserialize::<T>(&message_buffer[..]);
            match result{
                Ok(msg) => {
                    Some((address, msg))
                }
                err => {
                    log::warn!("Can't deserialize inc message.");
                    None
                }
            }
        }
    }
}
fn udp_send<T :  'static + Send + Serialize + DeserializeOwned>(socket: &mut UdpSocket, message: &T, address: &SocketAddr) {
    let msg_buffer = bincode::serialize(message).unwrap();
    socket.send_to(&msg_buffer, address).unwrap();
}
fn udp_send_connected<T :  'static + Send + Serialize + DeserializeOwned>(socket: &mut UdpSocket, message: &T) {
    let msg_buffer = bincode::serialize(message).unwrap();
    socket.send(&msg_buffer).unwrap();
}
enum DownMsg<T : Send>{
    SendFax(SocketAddr, T, /*Reliable*/bool),
    DropPlayer(SocketAddr)
}
enum UpMsg<T : Send>{
    PlayerConnected(SocketAddr),
    PlayerDisconnected(SocketAddr),
    NewFax(SocketAddr, T),
}

pub struct NetHubTop<T : Send > {
    down: Sender<DownMsg<T>>,
    pub up: Receiver<OutMsg<T>>,
    connected_players: Arc<RwLock<BiMap<PlayerID, SocketAddr>>>
}
impl<T :  'static + Send + Serialize + DeserializeOwned + Clone> NetHubTop<T> {
    pub fn kick(&mut self, player: PlayerID){
        let connnected = self.connected_players.write().unwrap();
        if let Some(address) = connnected.get_by_left(&player){
            self.down.send(DownMsg::DropPlayer(address.clone())).unwrap();
        }
    }
    pub fn recv(&mut self) -> OutMsg<T> {
        return self.up.recv().unwrap();
    }
    pub fn send_msg(&mut self, player: PlayerID, message: T, reliable: bool){
        let connnected = self.connected_players.write().unwrap();
        if let Some(address) = connnected.get_by_left(&player){
            self.down.send(DownMsg::SendFax(address.clone(), message, reliable)).unwrap();
        }
    }
    pub fn send_msg_all(&mut self, message: T, reliable: bool){
        let connnected = self.connected_players.write().unwrap();
        let players : Vec<PlayerID> = connnected.left_values().map(|item|{*item}).collect();
        std::mem::drop(connnected);
        for player in players{
            self.send_msg(player, message.clone(), reliable);
        }
    }
}
fn server_up_to_out<T :  'static + Send + Serialize + DeserializeOwned >
(up: Receiver<UpMsg<T>>, connected: Arc<RwLock<BiMap<PlayerID, SocketAddr>>>) -> Receiver<OutMsg<T>>{
    let (tx_out, rx_out) = unbounded();
    thread::spawn(move ||{
        let mut next_player_id = 1;
        let mut all_players_ever: BiMap<PlayerID, SocketAddr> = BiMap::new();
        let mut connected_players : Arc<RwLock<BiMap<PlayerID, SocketAddr>>> = connected;
        loop{
            let up_msg = up.recv().unwrap();
            match up_msg{
                UpMsg::PlayerConnected(address) => {
                    if !all_players_ever.contains_right(&address) {
                        all_players_ever.insert(next_player_id, address.clone());
                        next_player_id += 1;
                    }
                    let my_id = *all_players_ever.get_by_right(&address).unwrap();
                    let inserted = connected_players.write().unwrap().insert(my_id,address);
                    assert!(!inserted.did_overwrite()); // Shouldn't overwrite. Shouldn't get two 'connects' in a row.
                    tx_out.send(OutMsg::PlayerConnected(my_id)).unwrap();
                }
                UpMsg::PlayerDisconnected(address) => {
                    // Should be there. Shouldn't be two disconnects.
                    let (my_id, my_address) = connected_players.write().unwrap().remove_by_right(&address).unwrap();
                    tx_out.send(OutMsg::PlayerDisconnected(my_id)).unwrap();
                }
                UpMsg::NewFax(address, data) => {
                    // If fail, don't send anything.
                    if let Some(my_id) = connected_players.read().unwrap().get_by_right(&address){
                        tx_out.send(OutMsg::NewFax(*my_id, data)).unwrap();
                    }
                }
            }
        }
    });
    return rx_out;
}
enum InMsg{
}
#[derive(Debug, PartialEq)]
pub enum OutMsg<T>{
    PlayerConnected(PlayerID),
    PlayerDisconnected(PlayerID),
    NewFax(PlayerID, T),
}
pub fn start_server<T :  'static + Send + Serialize + DeserializeOwned >(host_address : String) -> NetHubTop<T>{
    let (down_sink, down_rec) = unbounded();
    let (up_sink, up_rec) = unbounded();

    let connected = Arc::new(RwLock::new(BiMap::new()));

    NetHubBottom{
        down: down_rec,
        up: up_sink,
        kill_switches: Default::default()
    }.start(host_address);

    NetHubTop{
        down: down_sink,
        up: server_up_to_out(up_rec, connected.clone()),
        connected_players: connected.clone()
    }
}



pub struct NetClientTop<T>{
    pub down: Sender<(T, bool)>,
    pub up: Receiver<T>,
}
impl<T : Send> NetClientTop<T>{
    pub fn send_msg(&mut self, message: T, reliable: bool){
        self.down.send((message, reliable)).unwrap();
    }
    pub fn recv(&mut self) -> T{
        return self.up.recv().unwrap();
    }
}
pub struct NetClientBottom<T>{
    conn_address: String,
    up: Sender<T>,
    down: Receiver<(T, bool)>,
}
impl<T :  'static + Send + Serialize + DeserializeOwned> NetClientBottom<T>{
    fn bind_sockets(&self) -> (UdpSocket, TcpStream){
        let target_tcp_address = SocketAddr::from_str(&self.conn_address).expect("Ill formed ip");

        let mut target_udp_address = target_tcp_address.clone();
        target_udp_address.set_port(target_tcp_address.port() + 1);

        loop{
            let tcp_stream;
            log::info!("Attempting to connect to {}", target_tcp_address.to_string());
            match TcpStream::connect(target_tcp_address){
                Err(error) => {
                    log::warn!("Failed to connect to server. Retrying ... ({})", error.to_string());
                    thread::sleep(Duration::from_millis(1000));
                    continue;
                }
                Ok(stream) => {
                    tcp_stream = stream;
                }
            }
            let udp_socket = UdpSocket::bind(tcp_stream.local_addr().unwrap()).expect("Client couldn't bind to socket.");

            udp_socket.connect(target_udp_address).expect("Client failed to connect UDP.");
            log::info!("Client using udp {:?}" , udp_socket.local_addr());
            log::info!("Connected to server on on tcp {} and udp on port +1", target_tcp_address);
            log::info!("");
            return (udp_socket, tcp_stream);
        }
    }
    fn start(mut self){
        let (mut udp, mut tcp) = self.bind_sockets();
        let mut udp_writer = udp.try_clone().unwrap();
        let mut tcp_writer = tcp.try_clone().unwrap();
        thread::spawn(move ||{
            loop{
                let (next_msg, reliable) = self.down.recv().unwrap();
                if reliable{
                    tcp_write(&mut tcp_writer, &next_msg);
                }else{
                    udp_send_connected(&mut udp_writer, &next_msg);
                }
            };
        });

        // Incoming messages go upwards:
        let upwards_tcp = self.up.clone();
        thread::spawn(move ||{
            loop{
                if let Some(next_tcp) = tcp_recv(&mut tcp){
                    upwards_tcp.send(next_tcp).unwrap();
                }
            }
        });
        // Incoming messages go upwards:
        let upwards_udp = self.up.clone();
        thread::spawn(move ||{
            loop{
                if let Some((address, next_tcp)) = udp_recv(&mut udp){
                    upwards_udp.send(next_tcp).unwrap();
                }
            }
        });
    }
}
