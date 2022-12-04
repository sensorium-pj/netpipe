use regex::Regex;
use std::cell::RefCell;
use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, WouldBlock};
use std::net::{TcpStream, ToSocketAddrs};
use std::{
    net::{TcpListener, UdpSocket},
    sync::{Arc, Mutex},
    thread,
};
use tungstenite::error::Error::{Io, Protocol};
use tungstenite::{accept, Message, WebSocket};

pub trait Broker {
    fn matches(&self, option: &String) -> bool;
    fn add_destination(&self, option: &String);
    fn send(&self, message: &String);
}

pub struct StdoutBroker {
    enabled: RefCell<bool>,
}

impl StdoutBroker {
    pub fn new() -> StdoutBroker {
        StdoutBroker {
            enabled: RefCell::new(false),
        }
    }
}

impl Broker for StdoutBroker {
    fn matches(&self, option: &String) -> bool {
        return option.eq("stdout");
    }

    fn add_destination(&self, _option: &String) {
        self.enabled.replace(true);
    }

    fn send(&self, message: &String) {
        if *self.enabled.borrow() {
            println!("{message}");
        }
    }
}

pub struct WebSocketBroker {
    sockets_list: RefCell<Vec<Arc<Mutex<Vec<WebSocket<TcpStream>>>>>>,
}

impl WebSocketBroker {
    pub fn new() -> WebSocketBroker {
        WebSocketBroker {
            sockets_list: RefCell::new(vec![]),
        }
    }
}

impl Broker for WebSocketBroker {
    fn matches(&self, option: &String) -> bool {
        return option.starts_with("ws://");
    }

    fn add_destination(&self, option: &String) {
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let sockets_ref = sockets.clone();
        let server = TcpListener::bind(get_host_port(option)).unwrap();
        thread::spawn(move || {
            for stream in server.incoming() {
                let stream = stream.unwrap();
                let socket = accept(stream).unwrap();
                socket.get_ref().set_nonblocking(true).unwrap();
                eprintln!("Connected: {}.", socket.get_ref().peer_addr().unwrap());
                sockets_ref.lock().unwrap().push(socket);
            }
        });
        self.sockets_list.borrow_mut().push(sockets);
    }

    fn send(&self, message: &String) {
        for sockets in self.sockets_list.borrow().iter() {
            sockets.lock().unwrap().retain_mut(|socket| {
                // println!("before socket.read");

                let read_result = socket.read_message();
                // println!("after socket.read");

                match read_result{
                //match socket.read_message() {
                    //
                    Ok(message) if message.is_close() => {
                        eprintln!("Socket closed: {}.", socket.get_ref().peer_addr().unwrap());
                        return false;
                    }
                    Ok(message) => panic!("[003] unknown message: {message}"),
                    Err(Io(e)) if e.kind() == WouldBlock => (),
                    Err(Io(e)) if e.kind() == ConnectionReset => {
                        eprintln!(
                            "Connection reset: {}.",
                            socket.get_ref().peer_addr().unwrap()
                        );
                        return false;
                    }
                    Err(Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    )) => {
                        eprintln!(
                            "Reset without closing handshake: {}.",
                            socket.get_ref().peer_addr().unwrap()
                        );
                        return false;
                    }
                    Err(e) => {
                        dbg!(e);
                        panic!("[001] encountered unknown error");
                    }
                    //

                }
                // println!("before socket.write");

                match socket.write_message(Message::text(message)) {
                    Ok(()) => (),
                    Err(Io(e)) if e.kind() == ConnectionAborted => {
                        eprintln!(
                            "Connection aborted: {}.",
                            socket.get_ref().peer_addr().unwrap()
                        );
                        return false;
                    }
                    Err(Io(e)) if e.kind() == ConnectionReset => {
                        eprintln!(
                            "Connection reset: {}.",
                            socket.get_ref().peer_addr().unwrap()
                        );
                        return false;
                    }
                    Err(Io(e)) if e.kind() == WouldBlock => {
                        println!("Err: WouldBlock in socket.write_message")
                    },

                    Err(Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    )) => {
                        eprintln!(
                            "Reset without closing handshake: {}.",
                            socket.get_ref().peer_addr().unwrap()
                        );
                        return false;
                    }

                    Err(e) => {
                        dbg!(e);
                        panic!("[002] encountered unknown error");
                    }
                };
                // println!("before socket.can_write");

                socket.can_write()
            })
        }
    }
}

fn get_host_port(uri: &String) -> String {
    return Regex::new(r"://(.*)/?")
        .unwrap()
        .captures(uri)
        .unwrap()
        .get(1)
        .unwrap()
        .as_str()
        .to_string();
}

pub struct UdpBroker {
    socket: UdpSocket,
    socket_v6: UdpSocket,
    destinations: RefCell<Vec<String>>,
}

impl UdpBroker {
    pub fn new() -> UdpBroker {
        let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
        let socket_v6 = UdpSocket::bind("[::]:0").unwrap();
        UdpBroker {
            socket,
            socket_v6,
            destinations: RefCell::new(vec![]),
        }
    }
}

impl Broker for UdpBroker {
    fn matches(&self, _option: &String) -> bool {
        true
    }

    fn add_destination(&self, option: &String) {
        self.destinations.borrow_mut().push(option.to_string());
    }

    fn send(&self, message: &String) {
        for addr in self.destinations.borrow().iter() {
            let addr = addr.to_socket_addrs().unwrap().into_iter().next().unwrap();
            if addr.is_ipv4() {
                self.socket.send_to(message.as_bytes(), addr).unwrap();
            } else {
                self.socket_v6.send_to(message.as_bytes(), addr).unwrap();
            }
        }
    }
}
