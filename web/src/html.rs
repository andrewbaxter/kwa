use std::future::Future;
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

pub fn button_text(text: &str, mut cb: impl FnMut() -> () + 'static) -> El {
    return el("button")
        .classes(&["button", "button_text"])
        .push(el("span").text(text))
        .on("click", move |_| cb());
}

pub fn button_icon(icon_id: &str, mut cb: impl FnMut() -> () + 'static) -> El {
    return el("button").classes(&["button", "button_icon"]).push(icon(icon_id)).on("click", move |_| cb());
}

pub fn button_image(image_src: &str, mut cb: impl FnMut() -> () + 'static) -> El {
    return el("button").classes(&["button", "button_icon"]).push(image(image_src)).on("click", move |_| cb());
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

pub fn async_result<
    F: Future<Output = Result<(), String>> + 'static,
>(pc: &mut ProcessingContext) -> (Prim<AsyncState>, Box<dyn Fn(F) -> ()>) {
    let result = Prim::new(pc, AsyncState::None);
    let eg = pc.eg();
    return (result.clone(), Box::new({
        let eg = eg.clone();
        move |f| {
            let eg = eg.clone();
            let result = result.clone();
            spawn_local(async move {
                eg.event(|pc| {
                    result.set(pc, AsyncState::InProgress);
                });
                let res = f.await;
                eg.event(|pc| {
                    match res {
                        Ok(_) => {
                            result.set(pc, AsyncState::None);
                        },
                        Err(e) => {
                            result.set(pc, AsyncState::Error(e));
                        },
                    };
                });
            });
        }
    }));
}

pub fn async_area(pc: &mut ProcessingContext, child: El, async_state: Prim<AsyncState>) -> El {
    let error = el("span").classes(&["error"]);
    let overlay = el("div").classes(&["async_overlay"]);
    return stack()
        .extend(vec![vbox().extend(vec![error.clone(), child]), overlay.clone()])
        .own(|_| link!((_pc = pc), (state = async_state), (), (error = error.clone(), overlay = overlay.clone()) {
            match state.get() {
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
        }));
}
