use actix_cors::Cors;
use actix_web::{web, App, Error, HttpResponse, HttpServer};
use futures::StreamExt;
use openssl::ssl::{SslAcceptor, SslFiletype, SslMethod};
use std::sync::mpsc::Sender;
use std::{io::stdin, net::UdpSocket, sync::mpsc, thread};
use tungstenite::connect;
use url::Url;

use crate::utils::get_host_port;

pub trait ReceiverCreator {
    fn matches(&self, option: &String) -> bool;
    fn create_receiver(&self, option: &String) -> Box<dyn Iterator<Item = String>>;
}

pub struct StdinReceiverCreator;
impl ReceiverCreator for StdinReceiverCreator {
    fn matches(&self, option: &String) -> bool {
        return option.eq("stdin");
    }

    fn create_receiver(&self, _option: &String) -> Box<dyn Iterator<Item = String>> {
        return Box::new(stdin().lines().into_iter().map(|l| l.unwrap()));
    }
}

pub struct HttpReceiverCreator;
impl ReceiverCreator for HttpReceiverCreator {
    fn matches(&self, option: &String) -> bool {
        return option.starts_with("https://");
    }

    fn create_receiver(&self, option: &String) -> Box<dyn Iterator<Item = String>> {
        let (tx, rx) = mpsc::channel();

        let addr = get_host_port(option);
        tokio::spawn(async {
            async fn handler(
                mut payload: web::Payload,
                tx: web::Data<Sender<String>>,
            ) -> Result<HttpResponse, Error> {
                while let Some(item) = payload.next().await {
                    let item = item.unwrap();
                    let message = String::from_utf8(item.to_vec()).unwrap();
                    if !message.is_empty() {
                        tx.send(message).unwrap();
                    }
                }
                Ok(HttpResponse::Ok().body("ok"))
            }

            let mut builder = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
            builder
                .set_private_key_file("./cert/server.key", SslFiletype::PEM)
                .unwrap();
            builder
                .set_certificate_chain_file("./cert/server.crt")
                .unwrap();

            let server = HttpServer::new(move || {
                App::new()
                    .wrap(Cors::permissive())
                    .app_data(web::Data::new(tx.clone()))
                    .service(web::resource("/").to(handler))
                    .default_service(web::to(|| HttpResponse::NotFound()))
            })
            .bind_openssl(addr, builder)
            .unwrap()
            .run();

            server.await
        });

        return Box::new(rx.into_iter());
    }
}

pub struct WebSocketReceiverCreator;
impl ReceiverCreator for WebSocketReceiverCreator {
    fn matches(&self, option: &String) -> bool {
        return option.starts_with("ws://");
    }

    fn create_receiver(&self, option: &String) -> Box<dyn Iterator<Item = String>> {
        let (mut socket, _) = connect(Url::parse(&option).unwrap()).unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || loop {
            let message = socket.read_message().unwrap();
            tx.send(message.into_text().unwrap()).unwrap();
        });
        return Box::new(rx.into_iter());
    }
}

pub struct UdpReceiverCreator;
impl ReceiverCreator for UdpReceiverCreator {
    fn matches(&self, _option: &String) -> bool {
        return true;
    }

    fn create_receiver(&self, option: &String) -> Box<dyn Iterator<Item = String>> {
        let socket = UdpSocket::bind(option).unwrap();

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || loop {
            let mut buf = [0; 8192];
            let buf_size = socket.recv(&mut buf).unwrap();
            let buf = &buf[..buf_size];
            tx.send(String::from_utf8(buf.to_vec()).unwrap()).unwrap();
        });
        return Box::new(rx.into_iter());
    }
}
