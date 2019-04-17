//! mpsc_requests rewritted for crossbeam, written by @stjepang (https://github.com/crossbeam-rs/crossbeam/issues/353#issuecomment-484013974)
//!
//! mpsc_requests is a small library built on top of crossbeam-channel but with
//! the addition of the consumer responding with a message to the producer.
//! Since the producer no longer only produces and the consumer no longer only consumes, the
//! Producer is renamed to [Requester] and the Consumer is renamed to [Responder].
//!
//! mpsc_requests is small and lean by only building on top of the rust standard library
//!
//! A perfect use-case for this library is single-threaded databases which need
//! to be accessed from multiple threads (such as SQLite)
//!
//! # Examples
//! For more examples, see the examples directory
//!
//! For even more examples see the tests in the tests directory
//!
//! ## Simple echo example
//! ```rust,run
//! use std::thread;
//! use aw_server::datastore::crossbeam_requests::channel;
//!
//! type RequestType = String;
//! type ResponseType = String;
//! let (responder, requester) = channel::<RequestType, ResponseType>();
//! thread::spawn(move || {
//!     responder.poll_loop(|mut req| {
//!         req.respond(req.body().clone());
//!     });
//! });
//! let msg = String::from("Hello");
//! let res = requester.request(msg.clone());
//! assert_eq!(res, msg);
//! ```

#![deny(missing_docs)]

use crossbeam_channel as cc;

/// Create a [Requester] and a [Responder] with a channel between them
///
/// The [Requester] can be cloned to be able to do requests to the same [Responder] from multiple
/// threads.
pub fn channel<Req, Res>() -> (Responder<Req, Res>, Requester<Req, Res>) {
    let (request_sender, request_receiver) = cc::unbounded::<Request<Req, Res>>();
    let c = Responder::new(request_receiver);
    let p = Requester::new(request_sender);
    return (c, p)

}

#[derive(Debug)]
/// Errors which can occur when a [Responder] handles a request
pub enum RequestError {
    /// Error occuring when channel from [Requester] to [Responder] is broken
    RecvError,
    /// Error occuring when channel from [Responder] to [Requester] is broken
    SendError
}
impl From<cc::RecvError> for RequestError {
    fn from(_err: cc::RecvError) -> RequestError {
        RequestError::RecvError
    }
}
impl<T> From<cc::SendError<T>> for RequestError {
    fn from(_err: cc::SendError<T>) -> RequestError {
        RequestError::SendError
    }
}

/// A object expected tois a request which is received from the [Responder] poll method
///
/// The request body can be obtained from the body() function and before being
/// dropped it needs to send a response with the respond() function.
/// Not doing a response on a request is considered a programmer error and will result in a panic
/// when the object gets dropped
pub struct Request<Req, Res> {
    request: Req,
    response_sender: cc::Sender<Res>,
    _responded: bool
}

impl<Req, Res> Request<Req, Res> {
    fn new(request: Req, response_sender: cc::Sender<Res>) -> Request<Req, Res> {
        Request {
            request: request,
            response_sender: response_sender,
            _responded: false,
        }
    }

    /// Get actual request data
    pub fn body(&self) -> &Req {
        &self.request
    }

    /// TODO
    pub fn respond(&mut self, response: Res) {
        if self._responded {
            panic!("Programmer error, same request cannot respond twice!");
        }
        match self.response_sender.send(response) {
            Ok(_) => (),
            Err(_e) => panic!("Request failed, send pipe was broken during request!")
        }
        self._responded = true;
    }
}

impl<Req, Res> Drop for Request<Req, Res> {
    fn drop(&mut self) {
        if !self._responded {
            panic!("Dropped request without responding, programmer error!");
        }
    }
}

/// A [Responder] listens to requests of a specific type and responds back to the [Requester]
pub struct Responder<Req, Res> {
    request_receiver: cc::Receiver<Request<Req, Res>>,
}

impl<Req, Res> Responder<Req, Res> {
    fn new(request_receiver: cc::Receiver<Request<Req, Res>>) -> Responder<Req, Res> {
        Responder {
            request_receiver: request_receiver,
        }
    }

    /// Poll if the [Responder] has received any requests.
    /// It then returns a Request which you need to call respond() on before dropping.
    /// Not calling respond is considered a programmer error and will result in a panic
    ///
    /// This call is blocking
    /// TODO: add try_poll
    pub fn poll(&self) -> Result<Request<Req, Res>, RequestError> {
        match self.request_receiver.recv() {
            Ok(r) => Ok(r),
            Err(_e) => Err(RequestError::RecvError)
        }
    }

    /// A shorthand for running poll with a closure for as long as there is one or more [Requester]s alive
    /// referencing this [Responder]
    pub fn poll_loop<F>(&self, mut f: F) where F: FnMut(Request<Req, Res>) {
        loop {
            match self.poll() {
                Ok(request) => f(request),
                Err(e) => match e {
                    // No more send channels open, quitting
                    RequestError::RecvError => break,
                    _ => panic!("This is a bug")
                }
            };
        }
    }
}

/// [Requester] has a connection to a [Responder] which it can send requests to
#[derive(Clone)]
pub struct Requester<Req, Res> {
    request_sender: cc::Sender<Request<Req, Res>>,
}

impl<Req, Res> Requester<Req, Res> {
    fn new(request_sender: cc::Sender<Request<Req, Res>>) -> Requester<Req, Res> {
        Requester {
            request_sender: request_sender,
        }
    }

    /// Send request to the connected [Responder]
    pub fn request(&self, request: Req) -> Res {
        let (response_sender, response_receiver) = cc::unbounded::<Res>();
        let full_request = Request::new(request, response_sender);
        self.request_sender.send(full_request).unwrap();
        response_receiver.recv().unwrap()
    }
}
