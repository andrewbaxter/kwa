use std::{
    fmt::Display,
    ops::{
        Sub,
        Add,
        Mul,
    },
    future::Future,
};
use gloo::storage::{
    LocalStorage,
    SessionStorage,
    Storage,
};
use lunk::{
    ProcessingContext,
    link,
};
use rooting::{
    ScopeValue,
    scope_any,
};
use serde::{
    de::DeserializeOwned,
    Serialize,
};
use wasm_bindgen_futures::spawn_local;

pub trait MoreMath {
    fn mix<T: Copy + Sub<Output = T> + Add<Output = T> + Mul<f64, Output = T>>(self, a: T, b: T) -> T;
}

impl MoreMath for f64 {
    fn mix<T: Copy + Sub<Output = T> + Add<Output = T> + Mul<f64, Output = T>>(self, a: T, b: T) -> T {
        return (b - a) * self.clamp(0., 1.) + a;
    }
}

#[macro_export]
macro_rules! bb{
    ($l: lifetime _; $($t: tt) *) => {
        $l: loop {
            #[allow(unreachable_code)] break {
                $($t) *
            };
        }
    };
    ($($t: tt) *) => {
        loop {
            #[allow(unreachable_code)] break {
                $($t) *
            };
        }
    };
}

#[macro_export]
macro_rules! log{
    ($t: literal $(, $a: expr) *) => {
        web_sys::console::log_1(&format!($t $(, $a) *).into());
    };
}

#[macro_export]
macro_rules! logd{
    ($t: literal $(, $a: expr) *) => {
        web_sys::console::log_1(&format!($t $(, $a) *).into());
    };
}

#[macro_export]
macro_rules! logn{
    ($t: literal $(, $a: expr) *) => {
    };
}

pub trait MyError<T> {
    fn log_ignore(self, context: &str);
    fn log_replace(self, context: &str, replacement: impl ToString) -> Result<T, String>;
    fn context(self, context: &str) -> Result<T, String>;
}

impl<T, E: Display> MyError<T> for Result<T, E> {
    fn log_ignore(self, context: &str) {
        match self {
            Ok(_) => { },
            Err(e) => {
                log!("{}: {}", context, e);
            },
        }
    }

    fn log_replace(self, context: &str, replacement: impl ToString) -> Result<T, String> {
        match self {
            Ok(v) => return Ok(v),
            Err(e) => {
                log!("{}: {}", context, e);
                return Err(replacement.to_string());
            },
        }
    }

    fn context(self, context: &str) -> Result<T, String> {
        match self {
            Ok(v) => return Ok(v),
            Err(e) => return Err(format!("{}: {}", context, e)),
        };
    }
}

impl<T> MyError<T> for Option<T> {
    fn log_ignore(self, context: &str) {
        match self {
            Some(_) => { },
            None => {
                log!("{}: missing value", context);
            },
        }
    }

    fn log_replace(self, context: &str, replacement: impl ToString) -> Result<T, String> {
        match self {
            Some(v) => return Ok(v),
            None => {
                log!("{}: missing value", context);
                return Err(replacement.to_string());
            },
        }
    }

    fn context(self, context: &str) -> Result<T, String> {
        match self {
            Some(v) => return Ok(v),
            None => return Err(format!("{}: missing value", context)),
        };
    }
}

pub fn local_state<
    T: PartialEq + Clone + Serialize + DeserializeOwned + 'static,
>(pc: &mut ProcessingContext, key: &'static str, default: impl Fn() -> T) -> (lunk::Prim<T>, ScopeValue) {
    let p =
        lunk::Prim::new(
            pc,
            LocalStorage::get::<String>(key).ok().and_then(|l| match serde_json::from_str::<T>(&l) {
                Ok(x) => Some(x),
                Err(e) => {
                    log!("Error parsing local storage setting [{}] with value [{}]: {}", key, l, e);
                    None
                },
            }).unwrap_or_else(default),
        );
    let drop = scope_any(link!((_pc = pc), (p = p.clone()), (), (key = key) {
        LocalStorage::set(key, serde_json::to_string(&*p.borrow()).unwrap()).unwrap();
    }));
    return (p, drop);
}

pub fn session_state<
    T: PartialEq + Clone + Serialize + DeserializeOwned + 'static,
>(pc: &mut ProcessingContext, key: &'static str, default: impl Fn() -> T) -> (lunk::Prim<T>, ScopeValue) {
    let p =
        lunk::Prim::new(
            pc,
            SessionStorage::get::<String>(key).ok().and_then(|l| match serde_json::from_str::<T>(&l) {
                Ok(x) => Some(x),
                Err(e) => {
                    log!("Error parsing session storage setting [{}] with value [{}]: {}", key, l, e);
                    None
                },
            }).unwrap_or_else(default),
        );
    let drop = scope_any(link!((_pc = pc), (p = p.clone()), (), (key = key) {
        SessionStorage::set(key, serde_json::to_string(&*p.borrow()).unwrap()).unwrap();
    }));
    return (p, drop);
}

pub fn bg<F: 'static + Future<Output = Result<(), String>>>(f: F) {
    spawn_local(async move {
        match f.await {
            Ok(_) => { },
            Err(e) => {
                log!("Background activity failed with error: {}", e);
            },
        };
    });
}

#[macro_export]
macro_rules! enum_unwrap{
    ($i: expr, $p: pat => $o: expr) => {
        match $i {
            $p => $o,
            _ => panic !(""),
        }
    };
}
