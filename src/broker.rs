use actix_cors::Cors;
use actix_web::web::Bytes;
use actix_web::{web, App, Error, HttpResponse, HttpServer};
use crossbeam::channel::{Receiver, Sender, TryRecvError};
use std::cell::RefCell;
use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, WouldBlock};
use std::net::{TcpStream, ToSocketAddrs};
use std::task::Poll;
use std::time::Duration;
use std::{
    net::{TcpListener, UdpSocket},
    sync::{Arc, Mutex},
    thread,
};
use tungstenite::error::Error::{Io, Protocol};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{accept, connect, Message, WebSocket};
use url::Url;

use crate::utils::get_host_port;

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

struct ReceiverStream {
    rx: web::Data<Receiver<String>>,
}

impl futures::Stream for ReceiverStream {
    type Item = Result<Bytes, Error>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.rx.try_recv() {
            Ok(value) => Poll::Ready(Some(Ok(value.into()))),
            Err(e) if e == TryRecvError::Empty => {
                let waker = cx.waker().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::new(0, 1_000_000));
                    waker.wake();
                });
                Poll::Pending
            }
            Err(e) if e == TryRecvError::Disconnected => Poll::Ready(None),
            Err(e) => panic!("Unknown error: {}", e),
        }
    }
}

pub struct HttpBroker {
    enabled: RefCell<bool>,
    rx: Receiver<String>,
    tx: Sender<String>,
}

impl HttpBroker {
    pub fn new() -> HttpBroker {
        let (tx, rx) = crossbeam::channel::unbounded();
        HttpBroker {
            rx,
            tx,
            enabled: RefCell::new(false),
        }
    }
}

impl Broker for HttpBroker {
    fn matches(&self, option: &String) -> bool {
        return option.starts_with("http://");
    }

    fn add_destination(&self, option: &String) {
        self.enabled.replace(true);
        let addr = get_host_port(option);
        let rx = self.rx.clone();
        tokio::spawn(async {
            async fn handler(rx: web::Data<Receiver<String>>) -> Result<HttpResponse, Error> {
                Ok(HttpResponse::Ok().streaming(ReceiverStream { rx }))
            }

            let server = HttpServer::new(move || {
                App::new()
                    .wrap(Cors::permissive())
                    .app_data(web::Data::new(rx.clone()))
                    .service(web::resource("/").to(handler))
                    .default_service(web::to(|| HttpResponse::NotFound()))
            })
            .bind(addr)
            .unwrap()
            .run();

            server.await
        });
    }

    fn send(&self, message: &String) {
        if *self.enabled.borrow() {
            self.tx.send(message.to_string() + "\n").unwrap();
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
                eprintln!("Connected: {}.", get_tcp_peer_addr(&socket));
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
                        eprintln!("Socket closed: {}.", get_tcp_peer_addr(socket));
                        return false;
                    }
                    Ok(message) => panic!("[003] unknown message: {message}"),
                    Err(Io(e)) if e.kind() == WouldBlock => (),
                    Err(Io(e)) if e.kind() == ConnectionReset => {
                        eprintln!("Connection reset: {}.", get_tcp_peer_addr(socket));
                        return false;
                    }
                    Err(Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    )) => {
                        eprintln!(
                            "Reset without closing handshake: {}.",
                            get_tcp_peer_addr(socket)
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
                        eprintln!("Connection aborted: {}.", get_tcp_peer_addr(socket));
                        return false;
                    }
                    Err(Io(e)) if e.kind() == ConnectionReset => {
                        eprintln!("Connection reset: {}.", get_tcp_peer_addr(socket));
                        return false;
                    }
                    Err(Io(e)) if e.kind() == WouldBlock => {
                        eprintln!("Err: WouldBlock in socket.write_message");
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
                            get_tcp_peer_addr(socket)
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

pub struct WebSocketClientBroker {
    socket_list: Arc<Mutex<Vec<WebSocket<MaybeTlsStream<TcpStream>>>>>,
}

fn get_peer_addr(socket: &WebSocket<MaybeTlsStream<TcpStream>>) -> String {
    match &socket.get_ref() {
        MaybeTlsStream::Plain(stream) => stream.peer_addr().unwrap().to_string(),
        #[cfg(feature = "native-tls")]
        MaybeTlsStream::NativeTls(stream) => stream.get_ref().peer_addr().unwrap().to_string(),
        #[cfg(feature = "__rustls-tls")]
        MaybeTlsStream::Rustls(stream) => stream.get_ref().peer_addr().unwrap().to_string(),
        &&_ => todo!(),
    }
}

fn get_tcp_peer_addr(socket: &WebSocket<TcpStream>) -> String {
    socket
        .get_ref()
        .peer_addr()
        .map_or("unknown address".to_owned(), |f| f.to_string())
}

impl WebSocketClientBroker {
    pub fn new() -> WebSocketClientBroker {
        WebSocketClientBroker {
            socket_list: Arc::new(Mutex::new(vec![])),
        }
    }
}

impl Broker for WebSocketClientBroker {
    fn matches(&self, option: &String) -> bool {
        return option.starts_with("ws-client://");
    }

    fn add_destination(&self, option: &String) {
        let url = format!("ws://{}", option.trim_start_matches("ws-client://"));
        let (socket, _) = connect(Url::parse(&url).unwrap()).unwrap();
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream.set_nonblocking(true).unwrap();
        }
        self.socket_list.lock().unwrap().push(socket);
    }

    fn send(&self, message: &String) {
        self.socket_list.lock().unwrap().retain_mut(|socket| {
            match socket.read_message() {
                Ok(message) if message.is_close() => {
                    eprintln!("Socket closed: {}.", get_peer_addr(socket));
                    return false;
                }
                Ok(message) => panic!("[003] unknown message: {message}"),
                Err(Io(e)) if e.kind() == WouldBlock => (),
                Err(Io(e)) if e.kind() == ConnectionReset => {
                    eprintln!("Connection reset: {}.", get_peer_addr(socket));
                    return false;
                }
                Err(Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake)) => {
                    eprintln!(
                        "Reset without closing handshake: {}.",
                        get_peer_addr(socket)
                    );
                    return false;
                }
                Err(e) => {
                    dbg!(e);
                    panic!("[001] encountered unknown error");
                }
            }
            match socket.write_message(Message::text(message)) {
                Ok(()) => true,
                Err(Io(e)) if e.kind() == ConnectionAborted => {
                    eprintln!("Connection aborted: {}.", get_peer_addr(socket));
                    return false;
                }
                Err(Io(e)) if e.kind() == ConnectionReset => {
                    eprintln!("Connection reset: {}.", get_peer_addr(socket));
                    return false;
                }
                Err(Io(e)) if e.kind() == WouldBlock => {
                    eprintln!("Err: WouldBlock in socket.write_message");
                    return false;
                }
                Err(Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake)) => {
                    eprintln!(
                        "Reset without closing handshake: {}.",
                        get_peer_addr(socket)
                    );
                    return false;
                }
                Err(e) => {
                    dbg!(e);
                    panic!("[002] encountered unknown error");
                }
            };
            socket.can_write()
        })
    }
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
