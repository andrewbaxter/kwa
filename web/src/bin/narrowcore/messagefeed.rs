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
    defer,
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
        FeedId,
    },
    log,
};
use super::{
    viewid::{
        FeedTime,
    },
    state::State,
    scrollentry::{
        EntryMap,
        FeedEntry,
    },
};

struct ChannelFeedMut {
    parent: Option<WeakInfiniscroll<Option<ChannelId>, FeedTime>>,
    server_time: Option<MessageId>,
    refreshing: Option<ScopeValue>,
}

pub struct ChannelFeed_ {
    id: ChannelId,
    state: State,
    mut_: RefCell<ChannelFeedMut>,
    entries: EntryMap,
}

#[derive(Clone)]
pub struct ChannelFeed(Rc<ChannelFeed_>);

impl ChannelFeed {
    pub fn new(state: &State, id: ChannelId) -> Self {
        return ChannelFeed(Rc::new(ChannelFeed_ {
            id: id,
            state: state.clone(),
            mut_: RefCell::new(ChannelFeedMut {
                parent: None,
                server_time: None,
                refreshing: None,
            }),
            entries: EntryMap::new(),
        }));
    }

    pub fn notify(&self, eg: EventGraph, id: DateMessageId) {
        if id.1.0 != self.0.id {
            return;
        }
        let want_after;
        {
            let mut mut_ = self.0.mut_.borrow_mut();
            if mut_.server_time.is_some() && id.1 <= mut_.server_time.unwrap() {
                return;
            }
            let Some(parent) = mut_.parent.clone().and_then(|p| p.upgrade()) else {
                return;
            };
            let Some(parent) = mut_.parent.clone().and_then(|p| p.upgrade()) else {
                return;
            };
            want_after = parent.want_after(Some(self.0.id.clone()), FeedTime {
                stamp: id.0,
                id: FeedId::Real(id.1.clone()),
            });
        }
        if let Some((pivot, count)) = want_after {
            self.request_after(eg.clone(), pivot, count);
        }
        self.trigger_refresh(eg);
    }

    pub fn channel(&self) -> &ChannelId {
        return &self.0.id;
    }

    pub fn trigger_refresh(&self, eg: EventGraph) {
        let mut mut_ = self.0.mut_.borrow_mut();
        if mut_.refreshing.is_some() {
            return;
        }
        mut_.refreshing = Some(spawn_rooted("pulling new channel events", {
            let self1 = self.clone();
            async move {
                let _cleanup = defer({
                    let self1 = self1.clone();
                    move || {
                        self1.0.mut_.borrow_mut().refreshing = None;
                    }
                });
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
                                let Some(e) = self1.0.entries.0.borrow_mut().get_mut(&FeedId::Real(entry.id)) else {
                                    continue;
                                };
                                e.0.text.set(pc, entry.text);
                            }
                        });
                        mut_.server_time = Some(server_time.unwrap());
                    }
                }
                return Ok(());
            }
        }));
    }
}

impl Feed<Option<ChannelId>, FeedTime> for ChannelFeed {
    fn set_parent(&self, parent: web::infiniscroll::WeakInfiniscroll<Option<ChannelId>, FeedTime>) {
        self.0.mut_.borrow_mut().parent = Some(parent);
    }

    fn request_around(&self, eg: EventGraph, time: FeedTime, count: usize) {
        bg("Channel feed - requesting messages around", {
            let self1 = self.clone();
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
                        if mut_.server_time.is_none() {
                            refresh = true;
                        } else if mut_.server_time.unwrap() != resp.server_time {
                            if resp.server_time < mut_.server_time.unwrap() {
                                mut_.server_time = Some(resp.server_time);
                            }
                            refresh = true;
                        } else {
                            refresh = false;
                        }
                    }
                    if refresh {
                        self1.trigger_refresh(pc.eg());
                    }
                });
                return Ok(());
            }
        });
    }

    fn request_before(&self, eg: EventGraph, time: FeedTime, count: usize) {
        bg("Channel feed, requesting messages before", {
            let self1 = self.clone();
            async move {
                let resp: S2USnapGetAroundResp = self1.0.state.0.world.req_get(U2SGet::SnapGetBefore {
                    id: enum_unwrap!(&time.id, FeedId:: Real(x) => x.clone()),
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
                        if mut_.server_time.is_none() {
                            refresh = true;
                        } else if mut_.server_time.unwrap() != resp.server_time {
                            if resp.server_time < mut_.server_time.unwrap() {
                                mut_.server_time = Some(resp.server_time);
                            }
                            refresh = true;
                        } else {
                            refresh = false;
                        }
                    }
                    if refresh {
                        self1.trigger_refresh(pc.eg());
                    }
                });
                return Ok(());
            }
        });
    }

    fn request_after(&self, eg: EventGraph, time: FeedTime, count: usize) {
        bg("Channel feed, requesting messages after", {
            let self1 = self.clone();
            async move {
                let resp: S2USnapGetAroundResp = self1.0.state.0.world.req_get(U2SGet::SnapGetAfter {
                    id: enum_unwrap!(&time.id, FeedId:: Real(x) => x.clone()),
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
                        if mut_.server_time.is_none() {
                            refresh = true;
                        } else if mut_.server_time.unwrap() != resp.server_time {
                            if resp.server_time < mut_.server_time.unwrap() {
                                mut_.server_time = Some(resp.server_time);
                            }
                            refresh = true;
                        } else {
                            refresh = false;
                        }
                    }
                    if refresh {
                        self1.trigger_refresh(pc.eg());
                    }
                });
                return Ok(());
            }
        });
    }
}
