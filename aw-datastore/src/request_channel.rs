//! Request/response channel between [`crate::Datastore`] handles and their
//! worker thread.
//!
//! Same shape as the `mpsc_requests` crate it replaces (and the same
//! crossbeam channels underneath), plus [`RequestReceiver::poll_timeout`]:
//! the worker must wake up to commit an open batch when no request arrives.

use std::time::Duration;

use crossbeam_channel as cc;

/// The other side of the channel is gone.
#[derive(Debug)]
pub(crate) struct Disconnected;

pub(crate) fn channel<Req, Res>() -> (RequestSender<Req, Res>, RequestReceiver<Req, Res>) {
    let (tx, rx) = cc::unbounded();
    (RequestSender { tx }, RequestReceiver { rx })
}

pub(crate) struct RequestSender<Req, Res> {
    tx: cc::Sender<(Req, ResponseSender<Res>)>,
}

// Not derived: that would require `Req: Clone` and `Res: Clone`.
impl<Req, Res> Clone for RequestSender<Req, Res> {
    fn clone(&self) -> Self {
        RequestSender {
            tx: self.tx.clone(),
        }
    }
}

impl<Req, Res> RequestSender<Req, Res> {
    pub(crate) fn request(&self, request: Req) -> Result<ResponseReceiver<Res>, Disconnected> {
        let (tx, rx) = cc::unbounded();
        self.tx
            .send((request, ResponseSender { tx }))
            .map_err(|_| Disconnected)?;
        Ok(ResponseReceiver { rx })
    }
}

pub(crate) struct ResponseReceiver<Res> {
    rx: cc::Receiver<Res>,
}

impl<Res> ResponseReceiver<Res> {
    pub(crate) fn collect(&self) -> Result<Res, Disconnected> {
        self.rx.recv().map_err(|_| Disconnected)
    }
}

pub(crate) struct RequestReceiver<Req, Res> {
    rx: cc::Receiver<(Req, ResponseSender<Res>)>,
}

impl<Req, Res> RequestReceiver<Req, Res> {
    /// Block until a request arrives. Fails once every sender is dropped.
    pub(crate) fn poll(&self) -> Result<(Req, ResponseSender<Res>), Disconnected> {
        self.rx.recv().map_err(|_| Disconnected)
    }

    /// Like [`Self::poll`], but returns `Ok(None)` if no request arrives
    /// within `timeout`.
    pub(crate) fn poll_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<(Req, ResponseSender<Res>)>, Disconnected> {
        match self.rx.recv_timeout(timeout) {
            Ok(request) => Ok(Some(request)),
            Err(cc::RecvTimeoutError::Timeout) => Ok(None),
            Err(cc::RecvTimeoutError::Disconnected) => Err(Disconnected),
        }
    }
}

pub(crate) struct ResponseSender<Res> {
    tx: cc::Sender<Res>,
}

impl<Res> ResponseSender<Res> {
    pub(crate) fn respond(&self, response: Res) {
        // Same contract as `mpsc_requests`: every caller waits for its response.
        if self.tx.send(response).is_err() {
            panic!("Response failed, send pipe was broken during request!")
        }
    }
}
