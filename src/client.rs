use crossbeam_channel::*;

struct Client<T : SocketClient>{
    user_logic : T
}


pub trait SocketClient<T:  'static + Send + Serialize + DeserializeOwned>{
    fn on_new_message<T>();
    fn on_tick<T>();
}

pub fn start_client<T>(conn_address: String, tick_delay: f32) -> NetClientTop<T> {
    let (down_sink, down_rec) = unbounded();
    let (up_sink, up_rec) = unbounded();

    let connected = Arc::new(RwLock::new(BiMap::<PlayerID, SocketAddr>::new()));

    let bottom = NetClientBottom{
        conn_address,
        down: down_rec,
        up: up_sink,
    }.start();
    NetClientTop{
        up: up_rec,
        down: down_sink
    }
}