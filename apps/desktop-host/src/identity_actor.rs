use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use crate::{DiagnosticKind, DiagnosticSpan, IdentityStore, PeerState, StoreError};

type RevocationJoin = Box<dyn FnOnce() -> bool + Send>;
type RevocationStart = Box<dyn FnOnce() -> RevocationJoin + Send>;

enum Request {
    Init(bool, SyncSender<Result<String, StoreError>>),
    Peer(SyncSender<Result<PeerState, StoreError>>),
    SetPeer(PeerState, SyncSender<Result<(), StoreError>>),
    Unpair(
        String,
        bool,
        Option<RevocationStart>,
        SyncSender<Result<(), StoreError>>,
    ),
    Stop,
}

struct Inner {
    sender: SyncSender<Request>,
    join: Mutex<Option<JoinHandle<()>>>,
    store: IdentityStore,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.sender.send(Request::Stop);
        if let Ok(join) = self.join.get_mut()
            && let Some(join) = join.take()
        {
            let _ = join.join();
        }
    }
}

#[derive(Clone)]
pub struct IdentityActor(Arc<Inner>);

impl IdentityActor {
    #[must_use]
    pub fn start(store: IdentityStore) -> Self {
        let diagnostic_store = store.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let join = thread::spawn(move || {
            while let Ok(request) = receiver.recv() {
                match request {
                    Request::Init(owner_controlled, reply) => {
                        let result = if owner_controlled {
                            store.init_for_owner()
                        } else {
                            store.init()
                        };
                        let _ = reply.send(result);
                    }
                    Request::Peer(reply) => {
                        let _ = reply.send(store.peer());
                    }
                    Request::SetPeer(peer, reply) => {
                        let _ = reply.send(store.set_peer(peer));
                    }
                    Request::Unpair(peer, owner_controlled, revoked, reply) => {
                        let revoke = || {
                            revoked.map_or_else(
                                || Box::new(|| true) as RevocationJoin,
                                |revoked| revoked(),
                            )
                        };
                        let result = if owner_controlled {
                            store.unpair_with_hook_for_owner(&peer, revoke)
                        } else {
                            store.unpair_with_hook(&peer, revoke)
                        };
                        let _ = reply.send(result);
                    }
                    Request::Stop => break,
                }
            }
        });
        Self(Arc::new(Inner {
            sender,
            join: Mutex::new(Some(join)),
            store: diagnostic_store,
        }))
    }

    pub fn init(&self) -> Result<String, StoreError> {
        self.call(|reply| Request::Init(false, reply))?
    }
    pub(crate) fn init_for_owner(&self) -> Result<String, StoreError> {
        self.call(|reply| Request::Init(true, reply))?
    }
    pub fn peer(&self) -> Result<PeerState, StoreError> {
        self.call(Request::Peer)?
    }
    pub(crate) fn set_peer(&self, peer: PeerState) -> Result<(), StoreError> {
        self.call(|reply| Request::SetPeer(peer, reply))?
    }
    pub fn unpair(&self, peer: String) -> Result<(), StoreError> {
        self.call(|reply| Request::Unpair(peer, false, None, reply))?
    }
    pub(crate) fn unpair_with_revocation(
        &self,
        peer: String,
        revoked: impl FnOnce() -> RevocationJoin + Send + 'static,
    ) -> Result<(), StoreError> {
        self.call(|reply| Request::Unpair(peer, true, Some(Box::new(revoked)), reply))?
    }

    pub(crate) fn stop_and_join(&self) -> Result<(), StoreError> {
        let mut join = self.0.join.lock().map_err(|_| StoreError::Io)?;
        if let Some(join) = join.take() {
            self.0
                .sender
                .send(Request::Stop)
                .map_err(|_| StoreError::Io)?;
            join.join().map_err(|_| StoreError::Io)?;
        }
        Ok(())
    }

    pub(crate) fn start_diagnostic(&self, kind: DiagnosticKind) -> Option<DiagnosticSpan> {
        self.0.store.start_diagnostic(kind)
    }

    fn call<T>(&self, request: impl FnOnce(SyncSender<T>) -> Request) -> Result<T, StoreError> {
        let (reply, result) = mpsc::sync_channel(1);
        self.0
            .sender
            .send(request(reply))
            .map_err(|_| StoreError::Io)?;
        result.recv().map_err(|_| StoreError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_serializes_identity_calls_and_joins() {
        let root =
            std::env::temp_dir().join(format!("deskkin-identity-actor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let actor = IdentityActor::start(IdentityStore::new(root));
        let public = actor.init().unwrap();
        assert_eq!(public.len(), 64);
        assert_eq!(actor.peer().unwrap(), PeerState::Unpaired);
        drop(actor);
    }
}
