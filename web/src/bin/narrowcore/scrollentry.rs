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
        FeedId,
    },
    log,
};
use super::{
    viewid::{
        FeedTime,
    },
    state::State,
};

pub struct EntryMap(pub Rc<RefCell<HashMap<FeedId, FeedEntry>>>);

impl EntryMap {
    pub fn new() -> Self {
        return Self(Rc::new(RefCell::new(HashMap::new())));
    }
}

pub struct MessageFeedEntry_ {
    pub entry_map: Weak<RefCell<HashMap<FeedId, FeedEntry>>>,
    pub id: FeedTime,
    pub text: Prim<String>,
}

pub struct FeedEntry(pub Rc<MessageFeedEntry_>);

impl FeedEntry {
    pub fn new(pc: &mut ProcessingContext, id: FeedTime, text: String, map: &EntryMap) -> Self {
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
