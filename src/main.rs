use niri_ipc::{Request, socket::Socket};

fn main() -> std::io::Result<()> {
    let mut socket = Socket::connect()?;
    let _reply = socket.send(Request::EventStream)?;

    let mut read_event = socket.read_events();
    loop {
        let event = read_event()?;
        println!("{event:?}");
    }
}
