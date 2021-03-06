use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use actix_rt::net::TcpStream;
use actix_service::{Service, ServiceFactory};
use futures_util::future::{err, ok, BoxFuture, Either, FutureExt, Ready};

use super::connect::{Address, Connect, Connection};
use super::error::ConnectError;

/// TCP connector service factory
#[derive(Debug)]
pub struct TcpConnectorFactory<T>(PhantomData<T>);

impl<T> TcpConnectorFactory<T> {
    pub fn new() -> Self {
        TcpConnectorFactory(PhantomData)
    }

    /// Create TCP connector service
    pub fn service(&self) -> TcpConnector<T> {
        TcpConnector(PhantomData)
    }
}

impl<T> Default for TcpConnectorFactory<T> {
    fn default() -> Self {
        TcpConnectorFactory(PhantomData)
    }
}

impl<T> Clone for TcpConnectorFactory<T> {
    fn clone(&self) -> Self {
        TcpConnectorFactory(PhantomData)
    }
}

impl<T: Address> ServiceFactory<Connect<T>> for TcpConnectorFactory<T> {
    type Response = Connection<T, TcpStream>;
    type Error = ConnectError;
    type Config = ();
    type Service = TcpConnector<T>;
    type InitError = ();
    type Future = Ready<Result<Self::Service, Self::InitError>>;

    fn new_service(&self, _: ()) -> Self::Future {
        ok(self.service())
    }
}

/// TCP connector service
#[derive(Default, Debug)]
pub struct TcpConnector<T>(PhantomData<T>);

impl<T> TcpConnector<T> {
    pub fn new() -> Self {
        TcpConnector(PhantomData)
    }
}

impl<T> Clone for TcpConnector<T> {
    fn clone(&self) -> Self {
        TcpConnector(PhantomData)
    }
}

impl<T: Address> Service<Connect<T>> for TcpConnector<T> {
    type Response = Connection<T, TcpStream>;
    type Error = ConnectError;
    #[allow(clippy::type_complexity)]
    type Future = Either<TcpConnectorResponse<T>, Ready<Result<Self::Response, Self::Error>>>;

    actix_service::always_ready!();

    fn call(&mut self, req: Connect<T>) -> Self::Future {
        let port = req.port();
        let Connect { req, addr, .. } = req;

        if let Some(addr) = addr {
            Either::Left(TcpConnectorResponse::new(req, port, addr))
        } else {
            error!("TCP connector: got unresolved address");
            Either::Right(err(ConnectError::Unresolved))
        }
    }
}

#[doc(hidden)]
/// TCP stream connector response future
pub struct TcpConnectorResponse<T> {
    req: Option<T>,
    port: u16,
    addrs: Option<VecDeque<SocketAddr>>,
    stream: Option<BoxFuture<'static, Result<TcpStream, io::Error>>>,
}

impl<T: Address> TcpConnectorResponse<T> {
    pub fn new(
        req: T,
        port: u16,
        addr: either::Either<SocketAddr, VecDeque<SocketAddr>>,
    ) -> TcpConnectorResponse<T> {
        trace!(
            "TCP connector - connecting to {:?} port:{}",
            req.host(),
            port
        );

        match addr {
            either::Either::Left(addr) => TcpConnectorResponse {
                req: Some(req),
                port,
                addrs: None,
                stream: Some(TcpStream::connect(addr).boxed()),
            },
            either::Either::Right(addrs) => TcpConnectorResponse {
                req: Some(req),
                port,
                addrs: Some(addrs),
                stream: None,
            },
        }
    }
}

impl<T: Address> Future for TcpConnectorResponse<T> {
    type Output = Result<Connection<T, TcpStream>, ConnectError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // connect
        loop {
            if let Some(new) = this.stream.as_mut() {
                match new.as_mut().poll(cx) {
                    Poll::Ready(Ok(sock)) => {
                        let req = this.req.take().unwrap();
                        trace!(
                            "TCP connector - successfully connected to connecting to {:?} - {:?}",
                            req.host(), sock.peer_addr()
                        );
                        return Poll::Ready(Ok(Connection::new(sock, req)));
                    }
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(err)) => {
                        trace!(
                            "TCP connector - failed to connect to connecting to {:?} port: {}",
                            this.req.as_ref().unwrap().host(),
                            this.port,
                        );
                        if this.addrs.is_none() || this.addrs.as_ref().unwrap().is_empty() {
                            return Poll::Ready(Err(err.into()));
                        }
                    }
                }
            }

            // try to connect
            let addr = this.addrs.as_mut().unwrap().pop_front().unwrap();
            this.stream = Some(TcpStream::connect(addr).boxed());
        }
    }
}
