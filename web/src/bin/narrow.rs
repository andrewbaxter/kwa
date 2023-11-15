use std::{
    panic,
    rc::{
        Rc,
        Weak,
    },
    cell::{
        RefCell,
        Cell,
    },
    collections::{
        HashMap,
        HashSet,
    },
    pin::{
        pin,
        Pin,
    },
    sync::atomic::AtomicI16,
    ops::Deref,
};
use caches::{
    WTinyLFUCache,
    Cache,
};
use chrono::{
    Utc,
    Duration,
    DateTime,
};
use futures::{
    channel::oneshot::{
        Receiver,
        Sender,
        channel,
    },
    Future,
};
use gloo::utils::window;
use js_sys::Object;
use lunk::{
    link,
    Prim,
    ProcessingContext,
    List,
    EventGraph,
};
use rooting::{
    set_root,
    el,
    El,
    ScopeValue,
};
use rooting_forms::Form;
use serde::{
    de::DeserializeOwned,
    Serialize,
    Deserialize,
};
use wasm_bindgen::{
    JsCast,
    JsValue,
};
use wasm_bindgen_futures::spawn_local;
use web::{
    infiniscroll::{
        Infiniscroll,
        Feed,
        WeakInfiniscroll,
        Entry,
    },
    html::{
        hbox,
        center_xy,
        vbox,
        stack,
        group,
        image,
        space,
        async_area,
        vscroll,
        bound_list,
        modal,
        dialpad,
        dialpad_button,
        button,
        icon,
        ElExt,
    },
    world::{
        World,
        ChannelId,
        MessageId,
        BrewId,
        U2SGet,
        S2UChannel,
        U2SPost,
        IdentityId,
        S2UGetAroundResp,
        S2UBrew,
    },
    util::{
        MyError,
        bg,
    },
    log,
    enum_unwrap,
};
use web_sys::{
    HtmlInputElement,
    Element,
    KeyboardEvent,
};

trait NowOrLaterKey: 'static + Clone + std::hash::Hash + Eq { }

impl<K: 'static + Clone + std::hash::Hash + Eq> NowOrLaterKey for K { }

trait NowOrLaterValue: 'static + Clone { }

impl<K: 'static + Clone> NowOrLaterValue for K { }

enum NowOrLater<K: NowOrLaterKey, V: NowOrLaterValue> {
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
struct Soft<K: NowOrLaterKey, V: NowOrLaterValue>(Rc<Soft_<K, V>>);

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
struct NowOrLaterer<K: NowOrLaterKey, V: NowOrLaterValue>(Rc<NowOrLaterer_<K, V>>);

impl<K: NowOrLaterKey, V: NowOrLaterValue> NowOrLaterer<K, V> {
    fn new(f: impl 'static + Fn(K) -> Pin<Box<dyn Future<Output = Result<V, String>>>>) -> Self {
        return NowOrLaterer(Rc::new(NowOrLaterer_ {
            unused: RefCell::new(WTinyLFUCache::<K, V>::builder().set_window_cache_size(100).finalize().unwrap()),
            used: Default::default(),
            get: Box::new(f),
            in_flight: Default::default(),
            pending: Default::default(),
        }));
    }

    fn get(&self, k: K) -> NowOrLater<K, V> {
        if let Some(v) = self.0.used.borrow().get(&k) {
            return NowOrLater::Now(Soft(v.upgrade().unwrap()));
        };
        if let Some(v) = self.0.unused.borrow_mut().remove(&k) {
            let out = Soft(Rc::new(Soft_ {
                noler: Rc::downgrade(&self.0),
                k: k.clone(),
                v: Some(v),
            }));
            self.0.used.borrow_mut().insert(k.clone(), Rc::downgrade(&out.0));
            return NowOrLater::Now(out);
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

    fn set(&self, k: K, v: V) -> Soft<K, V> {
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

fn nol_span<
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

#[derive(Clone)]
struct Message {
    id: MessageId,
    text: Prim<String>,
}

#[derive(Clone)]
struct Channel {
    id: ChannelId,
    name: Prim<String>,
}

#[derive(Clone)]
struct Brew {
    id: BrewId,
    name: Prim<String>,
    channels: List<ChannelId>,
}

#[derive(Clone)]
struct ChannelViewState {
    id: ChannelId,
    message: Prim<Option<MessageViewStateId>>,
}

#[derive(Clone)]
struct BrewViewState {
    id: BrewId,
    channel: Prim<Option<ChannelViewState>>,
}

#[derive(Clone)]
enum MessagesViewMode {
    Brew(BrewViewState),
    Channel(ChannelViewState),
}

#[derive(Clone)]
enum ViewState {
    Channels,
    Messages(Prim<MessagesViewMode>),
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
struct MessageViewStateId {
    id: MessageId,
    stamp: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ChannelViewStateId {
    id: ChannelId,
    message: Option<MessageViewStateId>,
}

#[derive(Serialize, Deserialize, Clone)]
struct BrewViewStateId {
    id: BrewId,
    channel: Option<ChannelViewStateId>,
}

#[derive(Serialize, Deserialize, Clone)]
enum ViewStateId {
    Brew(BrewViewStateId),
    Channel(ChannelViewStateId),
}

/// A non-session-persisted view state (menu, dialog, etc).
#[derive(Clone, PartialEq)]
pub enum TempViewState {
    AddChannel,
    AddChannelCreate,
    AddChannelLink,
}

struct State_ {
    local_id_base: i64,
    local_id_counter: AtomicI16,
    world: World,
    need_auth: Prim<bool>,
    view: Prim<ViewState>,
    temp_view: Prim<Option<TempViewState>>,
    brews: NowOrLaterer<BrewId, Brew>,
    channels: NowOrLaterer<ChannelId, Channel>,
}

#[derive(Clone)]
struct State(Rc<State_>);

fn new_channel_view_state(pc: &mut ProcessingContext, c: &ChannelViewStateId) -> ChannelViewState {
    return ChannelViewState {
        id: c.id.clone(),
        message: Prim::new(pc, match &c.message {
            Some(m) => Some(m.clone()),
            None => None,
        }),
    };
}

fn new_brew_view_state(pc: &mut ProcessingContext, b: &BrewViewStateId) -> BrewViewState {
    let c = match &b.channel {
        Some(c) => Some(new_channel_view_state(pc, c)),
        None => None,
    };
    return BrewViewState {
        id: b.id.clone(),
        channel: Prim::new(pc, c),
    };
}

fn set_view_(pc: &mut ProcessingContext, state: &State, id: &ViewStateId) -> bool {
    match &*state.0.view.borrow() {
        ViewState::Channels => {
            let m = match id {
                ViewStateId::Brew(b) => MessagesViewMode::Brew(new_brew_view_state(pc, b)),
                ViewStateId::Channel(c) => {
                    MessagesViewMode::Channel(new_channel_view_state(pc, c))
                },
            };
            let m1 = ViewState::Messages(Prim::new(pc, m));
            state.0.view.set(pc, m1);
            return true;
        },
        ViewState::Messages(mode) => {
            match (&*mode.borrow(), id) {
                (MessagesViewMode::Brew(b), ViewStateId::Brew(b1)) if b.id == b1.id => {
                    match (&*b.channel.borrow(), &b1.channel) {
                        (None, None) => {
                            return false;
                        },
                        (None, Some(c)) => {
                            let c2 = new_channel_view_state(pc, &c);
                            b.channel.set(pc, Some(c2));
                            return true;
                        },
                        (Some(_), None) => {
                            b.channel.set(pc, None);
                            return true;
                        },
                        (Some(c), Some(c1)) => {
                            match (&*c.message.borrow(), &c1.message) {
                                (None, None) => {
                                    return false;
                                },
                                (None, Some(m)) => {
                                    c.message.set(pc, Some(m.clone()));
                                    return true;
                                },
                                (Some(_), None) => {
                                    c.message.set(pc, None);
                                    return true;
                                },
                                (Some(m), Some(m1)) => {
                                    if m == m1 {
                                        return false;
                                    } else {
                                        c.message.set(pc, Some(m1.clone()));
                                        return true;
                                    }
                                },
                            }
                        },
                    }
                },
                (MessagesViewMode::Channel(c), ViewStateId::Channel(c1)) if c.id == c1.id => {
                    match (&*c.message.borrow(), &c1.message) {
                        (None, None) => {
                            return false;
                        },
                        (None, Some(m)) => {
                            c.message.set(pc, Some(m.clone()));
                            return true;
                        },
                        (Some(_), None) => {
                            c.message.set(pc, None);
                            return true;
                        },
                        (Some(m), Some(m1)) => {
                            if m == m1 {
                                return false;
                            } else {
                                c.message.set(pc, Some(m1.clone()));
                                return true;
                            }
                        },
                    }
                },
                (_, ViewStateId::Channel(c)) => {
                    let s = new_channel_view_state(pc, c);
                    mode.set(pc, MessagesViewMode::Channel(s));
                    return true;
                },
                (_, ViewStateId::Brew(b)) => {
                    let s = new_brew_view_state(pc, b);
                    mode.set(pc, MessagesViewMode::Brew(s));
                    return true;
                },
            }
        },
    }
}

fn set_view_message(pc: &mut ProcessingContext, state: &State, message_id: MessageViewStateId) {
    set_view(pc, state, match &*state.0.view.borrow() {
        ViewState::Channels => &ViewStateId::Channel(ChannelViewStateId {
            id: message_id.id.clone(),
            message: Some(message_id),
        }),
        ViewState::Messages(m) => {
            match &*m.borrow() {
                MessagesViewMode::Brew(b) => {
                    let brew = state.0.brews.get_immediate(b.id).unwrap();
                    if brew.channels.contains(message_id.id.0) {
                        &ViewStateId::Brew(BrewViewStateId {
                            id: b.id.clone(),
                            channel: b.channel.borrow().map(|c| ChannelViewStateId {
                                id: message_id.id.0.clone(),
                                message: Some(message_id),
                            }),
                        })
                    } else {
                        &ViewStateId::Channel(ChannelViewStateId {
                            id: message_id.id.0.clone(),
                            message: Some(message_id),
                        })
                    }
                },
                MessagesViewMode::Channel(c) => {
                    &ViewStateId::Channel(ChannelViewStateId {
                        id: message_id.id.0.clone(),
                        message: Some(message_id.clone()),
                    })
                },
            }
        },
    });
}

fn set_view(pc: &mut ProcessingContext, state: &State, id: &ViewStateId) {
    if set_view_(pc, state, id) {
        window()
            .history()
            .unwrap()
            .replace_state_with_url(&JsValue::NULL, "", Some(&format!("?{}", serde_json::to_string(id).unwrap())))
            .unwrap();
    }
}

fn set_view_nav(pc: &mut ProcessingContext, state: &State, id: &ViewStateId) {
    if set_view_(pc, state, id) {
        window()
            .history()
            .unwrap()
            .push_state_with_url(&JsValue::NULL, "", Some(&format!("?{}", serde_json::to_string(id).unwrap())))
            .unwrap();
    }
}

pub const ICON_MID_NONE: &'static str = "noimage_mid.png";

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Clone)]
enum FeedId {
    None,
    Local(String),
    Real(MessageId),
}

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct FeedTime(DateTime<Utc>, FeedId);

struct MessageFeedEntry {
    id: FeedTime,
    text: String,
}

impl Entry<FeedTime> for MessageFeedEntry {
    fn create_el(&self) -> El {
        return vbox().extend(vec![el("span").text(&self.id.0.to_rfc3339()), el("span").text(&self.text)]);
    }

    fn time(&self) -> FeedTime {
        return self.id.clone();
    }
}

#[derive(Clone)]
struct FeedParent {
    parent: WeakInfiniscroll<FeedTime>,
    feed_id: web::infiniscroll::FeedId,
}

struct OutboxFeed_ {
    mut_: RefCell<ChannelFeedMut>,
}

#[derive(Clone)]
struct OutboxFeed(Rc<OutboxFeed_>);

impl OutboxFeed {
    fn notify_local(&self, id: String) {
        let feed_parent = self.0.mut_.borrow().parent.as_ref().cloned().unwrap();
        let Some(parent) = feed_parent.parent.upgrade() else {
            return;
        };
        let time = FeedTime(Utc::now(), FeedId::Local(id));
        parent.notify_entry_after(feed_parent.feed_id, time.clone());
    }
}

impl Feed<FeedTime> for OutboxFeed {
    fn set_parent(
        &self,
        parent: web::infiniscroll::WeakInfiniscroll<FeedTime>,
        id_in_parent: web::infiniscroll::FeedId,
    ) {
        self.0.mut_.borrow_mut().parent = Some(FeedParent {
            parent: parent,
            feed_id: id_in_parent,
        });
    }

    fn request_around(&self, time: FeedTime, count: usize) { }

    fn request_before(&self, time: FeedTime, count: usize) { }

    fn request_after(&self, time: FeedTime, count: usize) { }
}

struct ChannelFeedMut {
    parent: Option<FeedParent>,
}

struct ChannelFeed {
    id: ChannelId,
    state: State,
    mut_: Rc<RefCell<ChannelFeedMut>>,
}

impl Feed<FeedTime> for ChannelFeed {
    fn set_parent(
        &self,
        parent: web::infiniscroll::WeakInfiniscroll<FeedTime>,
        id_in_parent: web::infiniscroll::FeedId,
    ) {
        self.mut_.borrow_mut().parent = Some(FeedParent {
            parent: parent,
            feed_id: id_in_parent,
        });
    }

    fn request_around(&self, time: FeedTime, count: usize) {
        bg({
            let channel = self.id.clone();
            let feed_parent = self.mut_.borrow().parent.as_ref().cloned().unwrap();
            let state = self.state.clone();
            async move {
                let resp: S2UGetAroundResp = state.0.world.req_get(U2SGet::GetAround {
                    channel: channel,
                    time: time.0,
                    count: count as u64,
                }).await?;
                let Some(parent) = feed_parent.parent.upgrade() else {
                    return Ok(());
                };
                parent.respond_entries_around(
                    feed_parent.feed_id,
                    time,
                    resp.entries.into_iter().map(|e| Rc::new(MessageFeedEntry {
                        id: FeedTime(e.time, FeedId::Real(e.id)),
                        text: e.text,
                    }) as Rc<dyn Entry<FeedTime>>).collect(),
                    resp.early_stop,
                    resp.late_stop,
                );
                return Ok(());
            }
        });
    }

    fn request_before(&self, time: FeedTime, count: usize) {
        bg({
            let feed_parent = self.mut_.borrow().parent.as_ref().cloned().unwrap();
            let state = self.state.clone();
            async move {
                let resp: S2UGetAroundResp = state.0.world.req_get(U2SGet::GetBefore {
                    id: enum_unwrap!(&time.1, FeedId:: Real(x) => x.clone()),
                    count: count as u64,
                }).await?;
                let Some(parent) = feed_parent.parent.upgrade() else {
                    return Ok(());
                };
                parent.respond_entries_before(
                    feed_parent.feed_id,
                    time,
                    resp.entries.into_iter().map(|e| Rc::new(MessageFeedEntry {
                        id: FeedTime(e.time, FeedId::Real(e.id)),
                        text: e.text,
                    }) as Rc<dyn Entry<FeedTime>>).collect(),
                    resp.early_stop,
                );
                return Ok(());
            }
        });
    }

    fn request_after(&self, time: FeedTime, count: usize) {
        bg({
            let feed_parent = self.mut_.borrow().parent.as_ref().cloned().unwrap();
            let state = self.state.clone();
            async move {
                let resp: S2UGetAroundResp = state.0.world.req_get(U2SGet::GetAfter {
                    id: enum_unwrap!(&time.1, FeedId:: Real(x) => x.clone()),
                    count: count as u64,
                }).await?;
                let Some(parent) = feed_parent.parent.upgrade() else {
                    return Ok(());
                };
                parent.respond_entries_after(
                    feed_parent.feed_id,
                    time,
                    resp.entries.into_iter().map(|e| Rc::new(MessageFeedEntry {
                        id: FeedTime(e.time, FeedId::Real(e.id)),
                        text: e.text,
                    }) as Rc<dyn Entry<FeedTime>>).collect(),
                    resp.late_stop,
                );
                return Ok(());
            }
        });
    }
}

async fn send(
    eg: &EventGraph,
    state: &State,
    feed: &OutboxFeed,
    textarea: &Element,
    channel: &ChannelId,
    reply: Option<&MessageId>,
) -> Result<(), String> {
    let textarea = textarea.dyn_ref::<HtmlInputElement>().unwrap();
    let text = textarea.value();
    let local_id =
        format!(
            "{}_{}",
            state.0.local_id_base,
            state.0.local_id_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
    let stamped_id = MessageViewStateId {
        id: x,
        stamp: Utc::now(),
    };
    feed.notify_local(stamped_id);
    state.0.world.req_post(U2SPost::Send {
        channel: channel.clone(),
        reply: reply.cloned(),
        local_id: local_id,
        body: text,
    }).await?;
    eg.event(|pc| {
        set_view_message(pc, state, stamped_id.clone());
        textarea.set_value("");
    });
    return Ok(());
}

fn build_compose(
    pc: &mut ProcessingContext,
    state: &State,
    messages: &Infiniscroll<FeedTime>,
    outbox_feed: &OutboxFeed,
    channel: &ChannelId,
    reply: Option<&MessageId>,
) -> El {
    let textarea = el("textarea");
    let compose = hbox();
    let (e, do_async) = async_area(pc, &compose);
    let do_async = Rc::new(do_async);
    compose.ref_classes(&["compose"]).ref_extend(vec![
        //. .
        el("div").classes(&["textarea_resizer"]).push(textarea.clone().on_resize({
            let messages = messages.clone();
            move |_el, _inline_size, block_size| {
                messages.set_padding_post(&format!("calc({}px + val(--pad))", block_size));
            }
        }).on("keypress", {
            let state = state.clone();
            let feed = outbox_feed.clone();
            let textarea = textarea.clone();
            let do_async = do_async.clone();
            let channel = channel.clone();
            let reply = reply.cloned();
            move |e| {
                let state = state.clone();
                let feed = feed.clone();
                let textarea = textarea.clone();
                let channel = channel.clone();
                let reply = reply.clone();
                let e = e.clone();
                (*do_async)(Box::pin(async move {
                    let e1 = e.dyn_ref::<KeyboardEvent>().unwrap();
                    if e1.key().to_ascii_lowercase() == "enter" && !e1.shift_key() {
                        e.stop_propagation();
                        send(&state, &feed, &textarea.raw(), &channel, reply.as_ref()).await?;
                    }
                    return Ok(());
                }))
            }
        })),
        button({
            let state = state.clone();
            let textarea = textarea.clone();
            let feed = outbox_feed.clone();
            let channel = channel.clone();
            let reply = reply.cloned();
            let do_async = do_async.clone();
            move || {
                let state = state.clone();
                let textarea = textarea.clone();
                let feed = feed.clone();
                let channel = channel.clone();
                let reply = reply.clone();
                (*do_async)(Box::pin(async move {
                    send(&state, &feed, &textarea.raw(), &channel, reply.as_ref()).await?;
                    return Ok(());
                }))
            }
        }).push(icon("send"))
    ]);
    return e;
}

fn build_add_channel_create(pc: &mut ProcessingContext, state: &State) -> El {
    #[derive(rooting_forms::Form)]
    struct Data {
        #[title("Name")]
        name: String,
    }

    let form = Data::new_form("");
    let inner = vbox();
    let (outer, async_do) = async_area(pc, &inner);
    inner.ref_extend(form.elements().elements).ref_extend(vec![hbox().extend(vec![
        //. .
        space(),
        button({
            let state = state.clone();
            let eg = pc.eg();
            move || {
                if let Ok(data) = form.parse() {
                    async_do({
                        let state = state.clone();
                        let eg = eg.clone();
                        Box::pin(async move {
                            let channel_id =
                                state
                                    .0
                                    .world
                                    .req_post_ret::<ChannelId>(U2SPost::ChannelCreate { name: data.name.clone() })
                                    .await?;
                            eg.event(|pc| {
                                let channel = Channel {
                                    id: channel_id.clone(),
                                    name: Prim::new(pc, data.name),
                                };
                                state.0.channels.set(channel_id.clone(), channel);
                                state.0.temp_view.set(pc, None);
                                set_view_nav(pc, &state, &ViewStateId::Channel(ChannelViewStateId {
                                    id: channel_id,
                                    message: None,
                                }));
                            });
                            return Ok(());
                        })
                    });
                }
            }
        }).push(el("span").text("Create")),
        space()
    ])]);
    return modal("Create channel", {
        let state = state.clone();
        let eg = pc.eg();
        move || eg.event(|pc| {
            state.0.temp_view.set(pc, None);
        })
    }, outer);
}

fn build_add_channel_link(pc: &mut ProcessingContext, state: &State) -> El {
    return modal("Add channel from link", {
        let state = state.clone();
        let eg = pc.eg();
        move || eg.event(|pc| {
            state.0.temp_view.set(pc, None);
        })
    }, dialpad().extend(vec![
        //. .
        dialpad_button("Create", "new", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelCreate));
            })
        }),
        dialpad_button("Paste", "text", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelLink));
            })
        })
    ]));
}

fn build_add_channel(pc: &mut ProcessingContext, state: &State) -> El {
    return modal("Add channel", {
        let state = state.clone();
        let eg = pc.eg();
        move || eg.event(|pc| {
            state.0.temp_view.set(pc, None);
        })
    }, dialpad().extend(vec![
        //. .
        dialpad_button("Create", "new", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelCreate));
            })
        }),
        dialpad_button("Paste", "text", {
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannelLink));
            })
        })
    ]));
}

fn build_channels(pc: &mut ProcessingContext, state: &State) -> El {
    fn build_channel(pc: &mut ProcessingContext, channel: &Channel) -> El {
        return hbox().extend(vec![el("span").bind_text(pc, &channel.name)]);
    }

    let list = el("div");
    bg({
        let state = state.clone();
        let eg = pc.eg();
        let list = list.clone();
        async move {
            let channels0: Vec<S2UChannel> = state.0.world.req_get(U2SGet::GetChannels).await?;
            eg.event(|pc| {
                let channels1: Vec<Soft<ChannelId, Channel>> = channels0.into_iter().map(|c| {
                    match state.0.channels.get(c.id.clone()) {
                        NowOrLater::Now(c) => c,
                        NowOrLater::Later(_) => {
                            state.0.channels.set(c.id.clone(), Channel {
                                id: c.id,
                                name: Prim::new(pc, c.name),
                            })
                        },
                    }
                }).collect();
                list.ref_clear();
                list.ref_extend(channels1.into_iter().map(|c| build_channel(pc, &*c)).collect());
            });
            return Ok(());
        }
    });
    return vbox().extend(vec![
        //. .
        hbox().extend(vec![button({
            let state = state.clone();
            let eg = pc.eg();
            move || eg.event(|pc| {
                state.0.temp_view.set(pc, Some(TempViewState::AddChannel));
            })
        }).push(icon("add"))]),
        vscroll().push(list)
    ]);
}

fn build_messages(pc: &mut ProcessingContext, state: &State, messages_view_state: &Prim<MessagesViewMode>) -> El {
    let outbox_feed = OutboxFeed(Rc::new(OutboxFeed_ { mut_: RefCell::new(ChannelFeedMut { parent: None }) }));
    let messages =
        Infiniscroll::new(
            FeedTime(Utc::now() + Duration::seconds(30), FeedId::None),
            vec![Box::new(outbox_feed.clone())],
        );
    return vbox().extend(vec![
        //. .
        stack().extend(vec![
            //. .
            messages.el(),
            hbox().extend(vec![button({
                let eg = pc.eg();
                let state = state.clone();
                move || eg.event(|pc| {
                    state.0.view.set(pc, ViewState::Channels);
                })
            }).push(icon("back")), group().own(|e| link!(
                //. .
                (pc = pc), (messages_view_state = messages_view_state.clone()), (), (e = e.weak(), state = state.clone()) {
                    let e = e.upgrade()?;
                    match &*messages_view_state.borrow() {
                        MessagesViewMode::Brew(b) => {
                            e.ref_clear();
                            e.extend(
                                vec![
                                    nol_span(pc, state.0.brews.get(b.id.clone()), |b| b.name.clone()),
                                    group().own(|e| link!(
                                        //. .
                                        (pc = pc), (agg_mode = b.channel.clone()), (), (e = e.weak(), state = state.clone()) {
                                            let e = e.upgrade()?;
                                            e.ref_clear();
                                            match &*agg_mode.borrow() {
                                                None => (),
                                                Some(c) => {
                                                    e.ref_push(
                                                        nol_span(
                                                            pc,
                                                            state.0.channels.get(c.id.clone()),
                                                            |c| c.name.clone(),
                                                        ),
                                                    );
                                                },
                                            }
                                        }))
                                ],
                            );
                        },
                        MessagesViewMode::Channel(c) => {
                            e.ref_clear();
                            e.ref_push(nol_span(pc, state.0.channels.get(c.id.clone()), |c| c.name.clone()));
                        },
                    }
                }))])
        ]),
        group().own(|e| link!(
            //. .
            (pc = pc),
            (view_mode = messages_view_state.clone()),
            (),
            (
                e = e.weak(),
                inner_own = Cell::new(None),
                state = state.clone(),
                messages = messages.clone(),
                outbox_feed = outbox_feed
            ) {
                let e = e.upgrade()?;
                inner_own.set(None);
                match &*view_mode.borrow() {
                    MessagesViewMode::Brew(g) => {
                        inner_own.set(Some(link!(
                            //. .
                            (pc = pc),
                            (agg_mode = g.channel.clone()),
                            (),
                            (
                                e = e.weak(),
                                inner_own = Cell::new(None),
                                state = state.clone(),
                                messages = messages.clone(),
                                outbox_feed = outbox_feed.clone()
                            ) {
                                let e = e.upgrade()?;
                                inner_own.set(None);
                                match &*agg_mode.borrow() {
                                    None => {
                                        // empty
                                    },
                                    Some(c) => {
                                        inner_own.set(Some(link!(
                                            //. .
                                            (pc = pc),
                                            (message = c.message.clone()),
                                            (),
                                            (
                                                e = e.weak(),
                                                state = state.clone(),
                                                messages = messages.clone(),
                                                outbox_feed = outbox_feed.clone(),
                                                c_id = c.id.clone()
                                            ) {
                                                let e = e.upgrade()?;
                                                match &*message.borrow() {
                                                    None => {
                                                        e.ref_clear();
                                                        e.ref_push(
                                                            build_compose(
                                                                pc,
                                                                state,
                                                                messages,
                                                                outbox_feed,
                                                                &c_id,
                                                                None,
                                                            ),
                                                        );
                                                    },
                                                    Some(m) => {
                                                        messages.clear_sticky();

                                                        // TODO set sticky
                                                        e.ref_clear();
                                                        e.ref_push(
                                                            build_compose(
                                                                pc,
                                                                state,
                                                                messages,
                                                                outbox_feed,
                                                                &c_id,
                                                                Some(m),
                                                            ),
                                                        );
                                                    },
                                                }
                                            }
                                        )));
                                    },
                                }
                            }
                        )));
                    },
                    MessagesViewMode::Channel(c) => {
                        e.ref_clear();
                        e.ref_push(build_compose(pc, state, messages, outbox_feed, &c.id, None));
                    },
                }
            }
        ))
    ]);
}

fn build_main(pc: &mut ProcessingContext, state: &State) -> El {
    return stack().extend(vec![
        //. .
        group().own(|e| link!(
            //. .
            (pc = pc), (view_state = state.0.view.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                e.ref_clear();
                match &*view_state.borrow() {
                    ViewState::Channels => {
                        e.ref_push(build_channels(pc, state));
                    },
                    ViewState::Messages(messages_view_state) => {
                        e.ref_push(build_messages(pc, state, &messages_view_state));
                    },
                }
            })),
        group().own(|e| link!(
            //. .
            (pc = pc), (temp_view_state = state.0.temp_view.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                if let Some(temp_view_state) = &*temp_view_state.borrow() {
                    e.ref_clear();
                    match temp_view_state {
                        TempViewState::AddChannel => {
                            e.ref_push(build_add_channel(pc, state));
                        },
                        TempViewState::AddChannelCreate => {
                            e.ref_push(build_add_channel_create(pc, state));
                        },
                        TempViewState::AddChannelLink => {
                            e.ref_push(build_add_channel_link(pc, state));
                        },
                    }
                }
            }))
    ]);
}

fn build_auth(pc: &mut ProcessingContext, state: &State) -> El {
    #[derive(rooting_forms::Form)]
    struct Login {
        #[title("Username")]
        username: String,
        #[title("Password")]
        password: rooting_forms::Password,
    }

    let form = Rc::new(Login::new_form(""));
    let inner = el("div");
    let (outer, do_async) = async_area(pc, &inner);
    inner.ref_extend(form.elements().elements).ref_push(hbox().extend(vec![space(), button({
        let eg = pc.eg();
        let state = state.clone();
        move || {
            let form = form.clone();
            let state = state.clone();
            let eg = eg.clone();
            do_async(Box::pin(async move {
                let Ok(details) = form.parse() else {
                    return Err(format!("There were issues with the information you provided."));
                };
                state.0.world.req_post(U2SPost::Auth {
                    username: details.username.clone(),
                    password: details.password.0,
                }).await.log_replace("Error authing", "There was an error logging in, please try again.")?;
                eg.event(|pc| {
                    state.0.need_auth.set(pc, false);
                });
                return Ok(());
            }))
        }
    }).push(el("span").text("Login"))]));
    return center_xy(vbox().push(image("logo.svg")).push(outer));
}

fn main() {
    panic::set_hook(Box::new(console_error_panic_hook::hook));
    let eg = lunk::EventGraph::new();
    eg.event(|pc| {
        let world = World::new();
        let state = State(Rc::new(State_ {
            local_id_base: Utc::now().timestamp_micros(),
            local_id_counter: AtomicI16::new(0),
            world: world.clone(),
            need_auth: Prim::new(pc, false),
            view: Prim::new(pc, ViewState::Channels),
            temp_view: Prim::new(pc, None),
            brews: NowOrLaterer::new({
                let world = world.clone();
                let eg = pc.eg();
                move |k: BrewId| {
                    let world = world.clone();
                    let eg = eg.clone();
                    Box::pin(async move {
                        let world = pin!(world);
                        let resp = world.req_get::<S2UBrew>(U2SGet::GetBrew(k.clone())).await?;
                        return eg.event(|pc| {
                            Ok(Brew {
                                name: Prim::new(pc, resp.name),
                                id: k.clone(),
                                channels: List::new(pc, resp.channels),
                            })
                        });
                    })
                }
            }),
            channels: NowOrLaterer::new({
                let world = world.clone();
                let eg = pc.eg();
                move |k: ChannelId| {
                    let world = world.clone();
                    let eg = eg.clone();
                    Box::pin(async move {
                        let world = pin!(world);
                        let resp = world.req_get::<S2UChannel>(U2SGet::GetChannel(k.clone())).await?;
                        return eg.event(|pc| {
                            Ok(Channel {
                                name: Prim::new(pc, resp.name),
                                id: k.clone(),
                            })
                        });
                    })
                }
            }),
        }));
        match (|| {
            let search =
                window()
                    .location()
                    .search()
                    .map_err(|e| e.dyn_ref::<Object>().unwrap().to_string())
                    .context("Error reading window location search")?;
            if search.is_empty() {
                return Ok(());
            }
            let query = search.strip_prefix("?").context("Missing ? at start of location search")?;
            let nav = serde_json::from_str(&query).context("Failed to parse query as json")?;
            set_view(pc, &state, &nav);
            return Ok(()) as Result<(), String>;
        })() {
            Ok(_) => { },
            Err(e) => {
                log!("Error parsing state from location, using default: {}", e);
            },
        };
        set_root(vec![
            //. .
            stack().own(|e| link!((pc = pc), (need_auth = state.0.need_auth.clone()), (), (e = e.weak(), state = state.clone()) {
                let e = e.upgrade()?;
                e.ref_clear();
                if *need_auth.borrow() {
                    e.ref_push(build_auth(pc, &state));
                } else {
                    e.ref_push(build_main(pc, &state));
                }
            }))
        ]);
    });
}
