pub mod socket;
pub mod stack;
pub mod virtio_net;

pub fn init() {
    let v = virtio_net::probe();
    stack::init(v);
}

pub fn poll_flush() {
    // 退出前多 poll 几轮把 FIN/数据发完
    for _ in 0..8 {
        stack::net_poll();
        stack::wait_ms(2);
    }
}
