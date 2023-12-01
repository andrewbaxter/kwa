//! This is a combined collection/cache with multiple purposes:
//!
//! 1. Ensure there's only one instance of live objects (channels/brews) so that updates
//!    are propagated to all users
//!
//! 2. Render immediately, rather than a blank frame before the async + post-async
//!    refresh, if there's data
//!
//! 3. Unified interface for async data
//!
//! 4. Caching
use std::{
    rc::{
        Weak,
        Rc,
    },
    ops::Deref,
    cell::RefCell,
    collections::{
        HashMap,
        HashSet,
    },
    pin::Pin,
};
use caches::{
    WTinyLFUCache,
    Cache,
};
use futures::{
    channel::oneshot::{
        Receiver,
        Sender,
        channel,
    },
    Future,
};
use wasm_bindgen_futures::spawn_local;
use crate::log;

pub trait NowOrLaterKey: 'static + Clone + std::hash::Hash + Eq { }

impl<K: 'static + Clone + std::hash::Hash + Eq> NowOrLaterKey for K { }

pub trait NowOrLaterValue: 'static + Clone { }

impl<K: 'static + Clone> NowOrLaterValue for K { }

pub enum NowOrLater<K: NowOrLaterKey, V: NowOrLaterValue> {
    Now(Hard<K, V>),
    Later(Receiver<Hard<K, V>>),
}

struct Hard_<K: NowOrLaterKey, V: NowOrLaterValue> {
    noler: Weak<NowOrLaterCollection_<K, V>>,
    k: K,
    v: Option<V>,
}

impl<K: NowOrLaterKey, V: NowOrLaterValue> Drop for Hard_<K, V> {
    fn drop(&mut self) {
        let Some(noler) = self.noler.upgrade() else {
            return;
        };
        noler.used.borrow_mut().remove(&self.k);
        noler.unused.borrow_mut().put(self.k.clone(), self.v.take().unwrap());
    }
}

/// Hard (from soft, vs weak) reference. Helper wrapper for managing live map +
/// dead cache to provide soft reference functionality. This must be kept around as
/// long as the value is in use.
#[derive(Clone)]
pub struct Hard<K: NowOrLaterKey, V: NowOrLaterValue>(Rc<Hard_<K, V>>);

impl<K: NowOrLaterKey, V: NowOrLaterValue> Deref for Hard<K, V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        return self.0.v.as_ref().unwrap();
    }
}

struct NowOrLaterCollection_<K: NowOrLaterKey, V: NowOrLaterValue> {
    unused: RefCell<WTinyLFUCache<K, V>>,
    used: RefCell<HashMap<K, Weak<Hard_<K, V>>>>,
    get: Box<dyn Fn(K) -> Pin<Box<dyn Future<Output = Result<V, String>>>>>,
    in_flight: RefCell<HashSet<K>>,
    pending: RefCell<HashMap<K, Vec<Sender<Hard<K, V>>>>>,
}

#[derive(Clone)]
pub struct NowOrLaterCollection<K: NowOrLaterKey, V: NowOrLaterValue>(Rc<NowOrLaterCollection_<K, V>>);

impl<K: NowOrLaterKey, V: NowOrLaterValue> NowOrLaterCollection<K, V> {
    pub fn new(f: impl 'static + Fn(K) -> Pin<Box<dyn Future<Output = Result<V, String>>>>) -> Self {
        return NowOrLaterCollection(Rc::new(NowOrLaterCollection_ {
            unused: RefCell::new(WTinyLFUCache::<K, V>::builder().set_window_cache_size(100).finalize().unwrap()),
            used: Default::default(),
            get: Box::new(f),
            in_flight: Default::default(),
            pending: Default::default(),
        }));
    }

    pub fn get_immediate(&self, k: &K) -> Option<Hard<K, V>> {
        if let Some(v) = self.0.used.borrow().get(&k) {
            return Some(Hard(v.upgrade().unwrap()));
        };
        if let Some(v) = self.0.unused.borrow_mut().remove(&k) {
            let out = Hard(Rc::new(Hard_ {
                noler: Rc::downgrade(&self.0),
                k: k.clone(),
                v: Some(v),
            }));
            self.0.used.borrow_mut().insert(k.clone(), Rc::downgrade(&out.0));
            return Some(out);
        }
        return None;
    }

    pub async fn get_async(&self, k: K) -> Result<Hard<K, V>, String> {
        match self.get(k) {
            NowOrLater::Now(v) => Ok(v),
            NowOrLater::Later(l) => {
                // Senders are owned by this, and this can't be dropped while get_async is
                // operating
                return Ok(l.await.unwrap());
            },
        }
    }

    pub fn get(&self, k: K) -> NowOrLater<K, V> {
        if let Some(v) = self.get_immediate(&k) {
            return NowOrLater::Now(v);
        }
        let (send, recv) = channel();
        self.0.pending.borrow_mut().entry(k.clone()).or_default().push(send);
        if self.0.in_flight.borrow_mut().insert(k.clone()) {
            let self1 = self.clone();
            spawn_local(async move {
                let getter = (self1.0.get)(k.clone());
                let v = getter.await;
                match v {
                    Ok(v) => {
                        self1.set(k, v);
                    },
                    Err(e) => {
                        self1.0.in_flight.borrow_mut().remove(&k);
                        log!("Error fetching remote value: {}", e);
                    },
                }
            });
        }
        return NowOrLater::Later(recv);
    }

    pub fn set(&self, k: K, v: V) -> Hard<K, V> {
        self.0.in_flight.borrow_mut().remove(&k);
        let out = Hard(Rc::new(Hard_ {
            noler: Rc::downgrade(&self.0),
            k: k.clone(),
            v: Some(v),
        }));
        self.0.used.borrow_mut().insert(k.clone(), Rc::downgrade(&out.0));
        for s in self.0.pending.borrow_mut().remove(&k).unwrap() {
            s.send(out.clone()).map_err(|_| ()).unwrap();
        }
        return out;
    }
}
