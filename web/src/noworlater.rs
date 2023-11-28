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
    Now(Soft<K, V>),
    Later(Receiver<Soft<K, V>>),
}

struct Soft_<K: NowOrLaterKey, V: NowOrLaterValue> {
    noler: Weak<NowOrLaterer_<K, V>>,
    k: K,
    v: Option<V>,
}

impl<K: NowOrLaterKey, V: NowOrLaterValue> Drop for Soft_<K, V> {
    fn drop(&mut self) {
        let Some(noler) = self.noler.upgrade() else {
            return;
        };
        noler.used.borrow_mut().remove(&self.k);
        noler.unused.borrow_mut().put(self.k.clone(), self.v.take().unwrap());
    }
}

/// Soft (vs weak) reference. Helper wrapper for managing live map + dead cache to
/// provide soft reference functionality.
#[derive(Clone)]
pub struct Soft<K: NowOrLaterKey, V: NowOrLaterValue>(Rc<Soft_<K, V>>);

impl<K: NowOrLaterKey, V: NowOrLaterValue> Deref for Soft<K, V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        return self.0.v.as_ref().unwrap();
    }
}

struct NowOrLaterer_<K: NowOrLaterKey, V: NowOrLaterValue> {
    unused: RefCell<WTinyLFUCache<K, V>>,
    used: RefCell<HashMap<K, Weak<Soft_<K, V>>>>,
    get: Box<dyn Fn(K) -> Pin<Box<dyn Future<Output = Result<V, String>>>>>,
    in_flight: RefCell<HashSet<K>>,
    pending: RefCell<HashMap<K, Vec<Sender<Soft<K, V>>>>>,
}

#[derive(Clone)]
pub struct NowOrLaterer<K: NowOrLaterKey, V: NowOrLaterValue>(Rc<NowOrLaterer_<K, V>>);

impl<K: NowOrLaterKey, V: NowOrLaterValue> NowOrLaterer<K, V> {
    pub fn new(f: impl 'static + Fn(K) -> Pin<Box<dyn Future<Output = Result<V, String>>>>) -> Self {
        return NowOrLaterer(Rc::new(NowOrLaterer_ {
            unused: RefCell::new(WTinyLFUCache::<K, V>::builder().set_window_cache_size(100).finalize().unwrap()),
            used: Default::default(),
            get: Box::new(f),
            in_flight: Default::default(),
            pending: Default::default(),
        }));
    }

    pub fn get_immediate(&self, k: &K) -> Option<Soft<K, V>> {
        if let Some(v) = self.0.used.borrow().get(&k) {
            return Some(Soft(v.upgrade().unwrap()));
        };
        if let Some(v) = self.0.unused.borrow_mut().remove(&k) {
            let out = Soft(Rc::new(Soft_ {
                noler: Rc::downgrade(&self.0),
                k: k.clone(),
                v: Some(v),
            }));
            self.0.used.borrow_mut().insert(k.clone(), Rc::downgrade(&out.0));
            return Some(out);
        }
        return None;
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

    pub fn set(&self, k: K, v: V) -> Soft<K, V> {
        self.0.in_flight.borrow_mut().remove(&k);
        let out = Soft(Rc::new(Soft_ {
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
