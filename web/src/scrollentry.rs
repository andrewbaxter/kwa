use std::{
    cell::RefCell,
    rc::{
        Rc,
        Weak,
    },
    collections::HashMap,
};
use chrono::{
    Utc,
    DateTime,
};
use lunk::{
    Prim,
    ProcessingContext,
};
use rooting::{
    El,
    el,
};
use serde::{
    Serialize,
    Deserialize,
};
use crate::{
    infiniscroll::{
        Entry,
    },
    html::{
        vbox,
        ElExt,
    },
    world::{
        FeedId,
    },
};

#[derive(Debug, Hash, PartialEq, Eq, PartialOrd, Ord, Clone, Serialize, Deserialize)]
pub struct FeedTime {
    pub stamp: DateTime<Utc>,
    pub id: FeedId,
}

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
