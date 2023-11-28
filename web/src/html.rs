use std::{
    future::Future,
    pin::Pin,
};
use lunk::{
    ProcessingContext,
    Prim,
    link,
    List,
};
use rooting::{
    el,
    El,
};
use wasm_bindgen_futures::spawn_local;
use crate::noworlater::{
    NowOrLaterKey,
    NowOrLaterValue,
    NowOrLater,
};

pub const CSS_HIDE: &'static str = "hide";

pub fn hbox() -> El {
    return el("div").classes(&["hbox"]);
}

pub fn vbox() -> El {
    return el("div").classes(&["vbox"]);
}

pub fn vscroll() -> El {
    return el("div").classes(&["vscroll"]);
}

pub fn stack() -> El {
    return el("div").classes(&["stack"]);
}

pub fn center_xy(child: El) -> El {
    return el("div").classes(&["center_xy"]).push(child);
}

/// Display: content
pub fn group() -> El {
    return el("group").classes(&["group"]);
}

pub fn image(src: &str) -> El {
    return el("img").attr("src", src);
}

pub fn space() -> El {
    return el("div").classes(&["space"]);
}

pub fn icon(name: &str) -> El {
    return el("span").classes(&["material-icons-outlined"]).text(name);
}

pub fn button(mut cb: impl FnMut() -> () + 'static) -> El {
    return el("button").classes(&["button"]).on("click", move |_| cb());
}

pub fn modal(title: &str, mut back_cb: impl FnMut() -> () + 'static, child: El) -> El {
    return el("div").classes(&["stack", "modal_veil"]).push(el("div").classes(&["modal"]).extend(vec![
        //. .
        el("button")
            .classes(&["button", "button_modal_back"])
            .extend(vec![icon("chevron_left"), el("span").text(title)])
            .on("click", move |_| back_cb()),
        child
    ]));
}

pub fn dialpad() -> El {
    return el("div").classes(&["dialpad"]);
}

pub fn dialpad_button(icon_id: &str, text: &str, mut cb: impl FnMut() -> () + 'static) -> El {
    return el("button")
        .classes(&["button", "button_dialpad"])
        .extend(vec![icon(icon_id), el("span").text(text)])
        .on("click", move |_| cb());
}

pub trait ElExt {
    fn ref_bind_text(&self, pc: &mut ProcessingContext, text: &Prim<String>) -> &Self;
    fn bind_text(self, pc: &mut ProcessingContext, text: &Prim<String>) -> Self;
}

impl ElExt for El {
    fn ref_bind_text(&self, pc: &mut ProcessingContext, text: &Prim<String>) -> &Self {
        self.ref_own(|e| link!((_pc = pc), (text = text), (), (e = e.weak()) {
            let e = e.upgrade()?;
            e.ref_text(&*text.borrow());
        }));
        return self;
    }

    fn bind_text(self, pc: &mut ProcessingContext, text: &Prim<String>) -> Self {
        self.ref_bind_text(pc, text);
        return self;
    }
}

pub fn bound_list<
    T: Clone + 'static,
>(pc: &mut ProcessingContext, list: &List<T>, map_child: impl Fn(&mut ProcessingContext, &T) -> El + 'static) -> El {
    return el("div").own(|e| link!((pc = pc), (list = list.clone()), (), (e = e.weak(), map_child = map_child) {
        let e = e.upgrade()?;
        for c in list.borrow_changes().iter() {
            e.ref_splice(c.offset, c.remove, c.add.iter().map(|e| map_child(pc, e)).collect());
        }
    }));
}

#[derive(Clone, PartialEq)]
pub enum AsyncState {
    None,
    InProgress,
    Error(String),
}

pub fn async_area(
    pc: &mut ProcessingContext,
    child: &El,
) -> (El, Box<dyn Fn(Pin<Box<dyn Future<Output = Result<(), String>>>>) -> ()>) {
    let async_state = Prim::new(pc, AsyncState::None);
    let error = el("span").classes(&["error"]);
    let overlay = el("div").classes(&["async_overlay"]);
    let e =
        stack()
            .extend(vec![vbox().extend(vec![error.clone(), child.clone()]), overlay.clone()])
            .own(
                |_| link!(
                    (_pc = pc),
                    (state = async_state.clone()),
                    (),
                    (error = error.clone(), overlay = overlay.clone()) {
                        match &*state.borrow() {
                            AsyncState::None => {
                                error.ref_classes(&[CSS_HIDE]);
                                overlay.ref_classes(&[CSS_HIDE]);
                            },
                            AsyncState::InProgress => {
                                error.ref_classes(&[CSS_HIDE]);
                                overlay.ref_remove_classes(&[CSS_HIDE]);
                            },
                            AsyncState::Error(text) => {
                                error.ref_classes(&[CSS_HIDE]);
                                error.ref_text(&text);
                                overlay.ref_classes(&[CSS_HIDE]);
                            },
                        }
                    }
                ),
            );
    let do_async = Box::new({
        let eg = pc.eg();
        move |f| {
            let eg = eg.clone();
            let async_state = async_state.clone();
            spawn_local(async move {
                eg.event(|pc| {
                    async_state.set(pc, AsyncState::InProgress);
                });
                let res = f.await;
                eg.event(|pc| {
                    match res {
                        Ok(_) => {
                            async_state.set(pc, AsyncState::None);
                        },
                        Err(e) => {
                            async_state.set(pc, AsyncState::Error(e));
                        },
                    };
                });
            });
        }
    });
    return (e, do_async);
}

pub fn nol_span<
    K: NowOrLaterKey,
    V: NowOrLaterValue,
>(pc: &mut ProcessingContext, nol: NowOrLater<K, V>, f: impl 'static + FnOnce(&V) -> Prim<String>) -> El {
    let out = el("span");
    match nol {
        NowOrLater::Now(v) => {
            out.ref_bind_text(pc, &f(&*v));
        },
        NowOrLater::Later(r) => {
            out.ref_text("...");
            spawn_local({
                let out = out.weak();
                let eg = pc.eg();
                async move {
                    let Some(out) = out.upgrade() else {
                        return;
                    };
                    let Ok(v) = r.await else {
                        return;
                    };
                    eg.event(|pc| {
                        out.ref_bind_text(pc, &f(&*v));
                    });
                }
            })
        },
    }
    return out;
}
