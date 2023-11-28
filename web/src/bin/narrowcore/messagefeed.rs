use std::{
    cell::RefCell,
    rc::{
        Rc,
        Weak,
    },
    collections::HashMap,
};
use chrono::Utc;
use lunk::{
    Prim,
    ProcessingContext,
    EventGraph,
};
use rooting::{
    El,
    el,
    ScopeValue,
};
use wasm_bindgen_futures::spawn_local;
use web::{
    infiniscroll::{
        Entry,
        WeakInfiniscroll,
        Feed,
        REQUEST_COUNT,
    },
    html::{
        vbox,
        ElExt,
    },
    util::{
        bg,
        spawn_rooted,
    },
    enum_unwrap,
    world::{
        S2USnapGetAroundResp,
        U2SGet,
        ChannelId,
        MessageId,
        DateMessageId,
        S2UEventsGetAfterResp,
    },
    log,
};
use super::{
    viewid::{
        FeedTime,
        FeedId,
    },
    state::State,
};

struct EntryMap(Rc<RefCell<HashMap<FeedId, FeedEntry>>>);

struct MessageFeedEntry_ {
    entry_map: Weak<RefCell<HashMap<FeedId, FeedEntry>>>,
    id: FeedTime,
    text: Prim<String>,
}

struct FeedEntry(Rc<MessageFeedEntry_>);

impl FeedEntry {
    fn new(pc: &mut ProcessingContext, id: FeedTime, text: String, map: &EntryMap) -> Self {
        return FeedEntry(Rc::new(MessageFeedEntry_ {
            entry_map: Rc::downgrade(&map.0),
            id: id,
            text: Prim::new(pc, text),
        }));
    }
}

impl Entry<FeedTime> for FeedEntry {
    fn create_el(&self, pc: &mut ProcessingContext) -> El {
        return vbox().extend(
            vec![el("span").text(&self.0.id.stamp.to_rfc3339()), el("span").bind_text(pc, &self.0.text)],
        );
    }

    fn time(&self) -> FeedTime {
        return self.0.id.clone();
    }
}

impl Drop for FeedEntry {
    fn drop(&mut self) {
        let Some(map) = self.0.entry_map.upgrade() else {
            return;
        };
        map.borrow_mut().remove(&self.0.id.id);
    }
}

struct OutboxFeed_ {
    mut_: RefCell<ChannelFeedMut>,
}

#[derive(Clone)]
pub struct OutboxFeed(Rc<OutboxFeed_>);

impl OutboxFeed {
    pub fn new() -> OutboxFeed {
        return OutboxFeed(Rc::new(OutboxFeed_ { mut_: RefCell::new(ChannelFeedMut { parent: None }) }));
    }

    pub fn notify_local(&self, channel: ChannelId, id: String) {
        let Some(parent) = self.0.mut_.borrow().parent.as_ref().cloned().unwrap().upgrade() else {
            return;
        };
        let time = FeedTime {
            stamp: Utc::now(),
            id: FeedId::Local(channel, id),
        };
        parent.notify_entry_after(None, time.clone());
    }
}

impl Feed<Option<ChannelId>, FeedTime> for OutboxFeed {
    fn set_parent(&self, parent: web::infiniscroll::WeakInfiniscroll<Option<ChannelId>, FeedTime>) {
        self.0.mut_.borrow_mut().parent = Some(parent);
    }

    fn request_around(&self, pc: &mut ProcessingContext, time: FeedTime, count: usize) { }

    fn request_before(&self, pc: &mut ProcessingContext, time: FeedTime, count: usize) { }

    fn request_after(&self, pc: &mut ProcessingContext, time: FeedTime, count: usize) { }
}

struct ChannelFeedMut {
    parent: Option<WeakInfiniscroll<Option<ChannelId>, FeedTime>>,
    server_time: MessageId,
    refreshing: Option<ScopeValue>,
}

pub struct ChannelFeed_ {
    id: ChannelId,
    state: State,
    mut_: Rc<RefCell<ChannelFeedMut>>,
    entries: EntryMap,
}

pub struct ChannelFeed(Rc<ChannelFeed_>);

impl ChannelFeed {
    pub fn notify(&self, pc: &mut ProcessingContext, id: DateMessageId) {
        if id.1.0 != self.0.id {
            return;
        }
        {
            let mut mut_ = self.0.mut_.borrow_mut();
            if id.1 <= mut_.server_time {
                return;
            }
            let Some(parent) = mut_.parent.clone().and_then(|p| p.upgrade()) else {
                return;
            };
            let Some(parent) = mut_.parent.clone().and_then(|p| p.upgrade()) else {
                return;
            };
            if let Some(pivot) = parent.want_after(Some(self.0.id.clone()), FeedTime {
                stamp: id.0,
                id: FeedId::Real(id.1.clone()),
            }) {
                self.request_after(pc, pivot, REQUEST_COUNT);
            }
        }
        self.trigger_refresh(pc);
    }

    pub fn trigger_refresh(&self, pc: &mut ProcessingContext) {
        let mut mut_ = self.0.mut_.borrow_mut();
        if mut_.refreshing.is_some() {
            return;
        }
        mut_.refreshing = Some(spawn_rooted({
            let self1 = self.clone();
            let eg = pc.eg();
            async move {
                let res = async move {
                    loop {
                        let resp = self1.0.state.0.world.req_get::<S2UEventsGetAfterResp>(U2SGet::EventsGetAfter {
                            id: self1.0.mut_.borrow().server_time.clone(),
                            count: REQUEST_COUNT as u64,
                        }).await?;
                        if resp.entries.is_empty() {
                            break;
                        }
                        {
                            let mut_ = self.0.mut_.borrow_mut();
                            let mut server_time = None;
                            eg.event(|pc| {
                                for entry in resp.entries {
                                    server_time = Some(entry.id);
                                    let Some(
                                        e
                                    ) = self1.0.entries.0.borrow_mut().get_mut(&FeedId::Real(entry.id)) else {
                                        continue;
                                    };
                                    e.0.text.set(pc, entry.text);
                                }
                            });
                            mut_.server_time = server_time.unwrap();
                        }
                    }
                    return Ok(()) as Result<(), String>;
                };
                self1.0.mut_.borrow_mut().refreshing = None;
                match res.await {
                    Ok(_) => { },
                    Err(e) => {
                        log!("Pulling new events failed: {}", e);
                    },
                };
            }
        }));
    }
}

impl Feed<Option<ChannelId>, FeedTime> for ChannelFeed {
    fn set_parent(&self, parent: web::infiniscroll::WeakInfiniscroll<Option<ChannelId>, FeedTime>) {
        self.0.mut_.borrow_mut().parent = Some(parent);
    }

    fn request_around(&self, pc: &mut ProcessingContext, time: FeedTime, count: usize) {
        bg({
            let self1 = self.clone();
            let eg = pc.eg();
            async move {
                let resp: S2USnapGetAroundResp = self1.0.state.0.world.req_get(U2SGet::SnapGetAround {
                    channel: self1.0.id.clone(),
                    time: time.stamp,
                    count: count as u64,
                }).await?;
                eg.event(|pc| {
                    let refresh;
                    {
                        let mut mut_ = self1.0.mut_.borrow_mut();
                        let Some(parent) = mut_.parent.and_then(|p| p.upgrade()) else {
                            return;
                        };
                        parent.respond_entries_around(
                            Some(self1.0.id.clone()),
                            time,
                            resp.entries.into_iter().map(|e| Rc::new(FeedEntry::new(pc, FeedTime {
                                stamp: e.time,
                                id: FeedId::Real(e.id),
                            }, e.text, &self1.0.entries)) as Rc<dyn Entry<FeedTime>>).collect(),
                            resp.early_stop,
                            resp.late_stop,
                        );
                        if mut_.server_time != resp.server_time {
                            if resp.server_time < mut_.server_time {
                                mut_.server_time = resp.server_time;
                            }
                            refresh = true;
                        } else {
                            refresh = false;
                        }
                    }
                    if refresh {
                        self1.trigger_refresh(pc);
                    }
                });
                return Ok(());
            }
        });
    }

    fn request_before(&self, pc: &mut ProcessingContext, time: FeedTime, count: usize) {
        bg({
            let self1 = self.clone();
            let eg = pc.eg();
            async move {
                let resp: S2USnapGetAroundResp = self1.0.state.0.world.req_get(U2SGet::SnapGetBefore {
                    id: enum_unwrap!(&time.id, FeedId:: Real(x) =>* x),
                    count: count as u64,
                }).await?;
                eg.event(|pc| {
                    let refresh;
                    {
                        let mut mut_ = self1.0.mut_.borrow_mut();
                        let Some(parent) = mut_.parent.and_then(|p| p.upgrade()) else {
                            return;
                        };
                        parent.respond_entries_before(
                            &Some(self.0.id.clone()),
                            &time,
                            resp.entries.into_iter().map(|e| Rc::new(FeedEntry::new(pc, FeedTime {
                                stamp: e.time,
                                id: FeedId::Real(e.id),
                            }, e.text, &self1.0.entries)) as Rc<dyn Entry<FeedTime>>).collect(),
                            resp.early_stop,
                        );
                        if mut_.server_time != resp.server_time {
                            if resp.server_time < mut_.server_time {
                                mut_.server_time = resp.server_time;
                            }
                            refresh = true;
                        } else {
                            refresh = false;
                        }
                    }
                    if refresh {
                        self1.trigger_refresh(pc);
                    }
                });
                return Ok(());
            }
        });
    }

    fn request_after(&self, pc: &mut ProcessingContext, time: FeedTime, count: usize) {
        bg({
            let self1 = self.clone();
            let eg = pc.eg();
            async move {
                let resp: S2USnapGetAroundResp = self1.0.state.0.world.req_get(U2SGet::SnapGetAfter {
                    id: enum_unwrap!(&time.id, FeedId:: Real(x) =>* x),
                    count: count as u64,
                }).await?;
                eg.event(|pc| {
                    let refresh;
                    {
                        let mut mut_ = self1.0.mut_.borrow_mut();
                        let Some(parent) = mut_.parent.and_then(|p| p.upgrade()) else {
                            return;
                        };
                        parent.respond_entries_after(
                            &Some(self.0.id.clone()),
                            &time,
                            resp.entries.into_iter().map(|e| Rc::new(FeedEntry::new(pc, FeedTime {
                                stamp: e.time,
                                id: FeedId::Real(e.id),
                            }, e.text, &self1.0.entries)) as Rc<dyn Entry<FeedTime>>).collect(),
                            resp.late_stop,
                        );
                        if mut_.server_time != resp.server_time {
                            if resp.server_time < mut_.server_time {
                                mut_.server_time = resp.server_time;
                            }
                            refresh = true;
                        } else {
                            refresh = false;
                        }
                    }
                    if refresh {
                        self1.trigger_refresh(pc);
                    }
                });
                return Ok(());
            }
        });
    }
}
