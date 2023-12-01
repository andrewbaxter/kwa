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
use gloo::utils::format::JsValueSerdeExt;
use indexed_db_futures::{
    IdbQuerySource,
    IdbIndex,
};
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
use wasm_bindgen::JsValue;
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
        MyErrorDomException,
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
    dbmodel::{
        TABLE_OUTBOX,
        OutboxEntryV1,
        OutboxEntry,
        TABLE_OUTBOX_INDEX_STAMP,
        from_outbox,
        outbox_key,
    },
    bb,
};
use web_sys::{
    IdbCursorDirection,
    IdbKeyRange,
    IdbCursor,
};
use crate::narrowcore::scrollentry::{
    FeedEntry,
    EntryMap,
};
use super::{
    viewid::{
        FeedTime,
    },
    state::State,
};

struct OutboxFeedMut {
    parent: Option<WeakInfiniscroll<Option<ChannelId>, FeedTime>>,
}

struct OutboxFeed_ {
    state: State,
    mut_: RefCell<OutboxFeedMut>,
}

#[derive(Clone)]
pub struct OutboxFeed(Rc<OutboxFeed_>);

impl OutboxFeed {
    pub fn new(state: &State) -> OutboxFeed {
        return OutboxFeed(Rc::new(OutboxFeed_ {
            state: state.clone(),
            mut_: RefCell::new(OutboxFeedMut { parent: None }),
        }));
    }

    pub fn notify(&self, eg: EventGraph, channel: ChannelId, id: String) {
        let pivot;
        let count;
        {
            let Some(parent) = self.0.mut_.borrow().parent.as_ref().cloned().unwrap().upgrade() else {
                return;
            };
            let time = FeedTime {
                stamp: Utc::now(),
                id: FeedId::Local(channel, id),
            };
            let Some((pivot1, count)) = parent.want_after(None, time.clone()) else {
                return;
            };
            pivot = pivot1;
        }
        self.request_after(eg, pivot, count);
    }
}

async fn read_before<
    'x,
>(time_index: &IdbIndex<'x>, pivot: DateTime<Utc>, count: usize) -> Result<(bool, Vec<OutboxEntry>), String> {
    let mut before = vec![];
    let Some(
        cursor
    ) = time_index.open_cursor_with_range_and_direction(
        &IdbKeyRange::upper_bound_with_open(
            &<JsValue as JsValueSerdeExt>::from_serde(&pivot).unwrap(),
            true,
        ).unwrap(),
        IdbCursorDirection::Prev
    ).context("Failed to open outbox cursor") ?.await.context("Error waiting for cursor") ? else {
        return Ok((true, before));
    };
    loop {
        if before.len() >= count {
            return Ok((false, before));
        }
        if !cursor
            .continue_cursor()
            .context("Error moving cursor forward")?
            .await
            .context("Error retrieving cursor advance result")? {
            return Ok((false, before));
        }
        before.push(from_outbox(&cursor.value()));
    }
}

fn finish_entries(pc: &mut ProcessingContext, v: Vec<OutboxEntry>) -> Vec<Rc<dyn Entry<FeedTime>>> {
    return v.into_iter().map(|e| match e {
        OutboxEntry::V1(e) => Rc::new(FeedEntry::new(pc, FeedTime {
            stamp: e.stamp,
            id: match e.resolved_id {
                Some(id) => FeedId::Real(id),
                None => FeedId::Local(e.channel, e.local_id),
            },
        }, e.body, &EntryMap::new())) as Rc<dyn Entry<FeedTime>>,
    }).collect();
}

impl Feed<Option<ChannelId>, FeedTime> for OutboxFeed {
    fn set_parent(&self, parent: web::infiniscroll::WeakInfiniscroll<Option<ChannelId>, FeedTime>) {
        self.0.mut_.borrow_mut().parent = Some(parent);
    }

    fn request_around(&self, eg: EventGraph, time: FeedTime, count: usize) {
        bg("Outbox feed, request around", {
            let self1 = self.clone();
            async move {
                let txn =
                    self1
                        .0
                        .state
                        .0
                        .db
                        .transaction_on_multi_with_mode(&[TABLE_OUTBOX], web_sys::IdbTransactionMode::Readonly)
                        .context("Failed to start transaction")?;
                let time_index =
                    txn
                        .object_store(TABLE_OUTBOX)
                        .context("Failed to get outbox")?
                        .index(TABLE_OUTBOX_INDEX_STAMP)
                        .context("Failed to get outbox stamp index")?;

                // Get elements before pivot
                let mut early_stop = true;
                let mut before = vec![];

                bb!{
                    'read_done _;
                    let Some(
                        cursor
                    ) = time_index.open_cursor_with_range_and_direction(
                        &IdbKeyRange::upper_bound_with_open(
                            &<JsValue as JsValueSerdeExt>::from_serde(&time).unwrap(),
                            true,
                        ).unwrap(),
                        IdbCursorDirection::Prev
                    ).context("Failed to open outbox cursor") ?.await.context("Error waiting for cursor") ? else {
                        break 'read_done;
                    };
                    loop {
                        if before.len() >= count {
                            early_stop = false;
                            break 'read_done;
                        }
                        if !cursor
                            .continue_cursor()
                            .context("Error moving cursor forward")?
                            .await
                            .context("Error retrieving cursor advance result")? {
                            break 'read_done;
                        }
                        before.push(from_outbox(&cursor.value()));
                    }
                }

                before.reverse();

                // Get elements including and after pivot
                let mut late_stop = true;
                let mut after_including: Vec<OutboxEntry> = vec![];

                bb!{
                    'read_done _;
                    let Some(
                        cursor
                    ) = time_index.open_cursor_with_range_and_direction(
                        &IdbKeyRange::lower_bound(
                            &<JsValue as JsValueSerdeExt>::from_serde(&time.stamp).unwrap(),
                        ).unwrap(),
                        IdbCursorDirection::Next
                    ).context("Failed to open outbox cursor") ?.await.context("Error waiting for cursor") ? else {
                        break 'read_done;
                    };
                    loop {
                        if after_including.len() >= count + 1 {
                            late_stop = false;
                            break 'read_done;
                        }
                        if !cursor
                            .continue_cursor()
                            .context("Error moving cursor forward")?
                            .await
                            .context("Error retrieving cursor advance result")? {
                            break 'read_done;
                        }
                        after_including.push(from_outbox(&cursor.value()));
                    }
                }

                // Finish read
                txn.await.into_result().context("Failed to commit transaction")?;

                // Combine and send
                let mut all = before;
                all.extend(after_including);
                eg.event(|pc| {
                    let mut mut_ = self1.0.mut_.borrow_mut();
                    let Some(parent) = mut_.parent.and_then(|p| p.upgrade()) else {
                        return;
                    };
                    parent.respond_entries_around(None, time, finish_entries(pc, all), early_stop, late_stop);
                });
                return Ok(());
            }
        });
    }

    fn request_before(&self, eg: EventGraph, time: FeedTime, count: usize) {
        bg("Outbox feed, request before", {
            let self1 = self.clone();
            async move {
                let txn =
                    self1
                        .0
                        .state
                        .0
                        .db
                        .transaction_on_multi_with_mode(&[TABLE_OUTBOX], web_sys::IdbTransactionMode::Readonly)
                        .context("Failed to start transaction")?;
                let outbox = txn.object_store(TABLE_OUTBOX).context("Failed to get outbox")?;

                // Get entries
                let mut early_stop = true;
                let mut before = vec![];

                bb!{
                    'read_done _;
                    let Some(
                        cursor
                    ) = outbox.open_cursor_with_range_and_direction(
                        &IdbKeyRange::upper_bound_with_open(
                            &outbox_key(&enum_unwrap!(time.id, FeedId:: Local(_, id) => id)),
                            true,
                        ).unwrap(),
                        IdbCursorDirection::Prev
                    ).context("Failed to open outbox cursor") ?.await.context("Error waiting for cursor") ? else {
                        break 'read_done;
                    };
                    loop {
                        if before.len() >= count {
                            early_stop = false;
                            break 'read_done;
                        }
                        if !cursor
                            .continue_cursor()
                            .context("Error moving cursor forward")?
                            .await
                            .context("Error retrieving cursor advance result")? {
                            break 'read_done;
                        }
                        before.push(from_outbox(&cursor.value()));
                    }
                }

                // Finish read
                txn.await.into_result().context("Failed to commit transaction")?;

                // Combine and send
                eg.event(|pc| {
                    let mut mut_ = self1.0.mut_.borrow_mut();
                    let Some(parent) = mut_.parent.and_then(|p| p.upgrade()) else {
                        return;
                    };
                    parent.respond_entries_before(&None, &time, finish_entries(pc, before), early_stop);
                });
                return Ok(());
            }
        });
    }

    fn request_after(&self, eg: EventGraph, time: FeedTime, count: usize) {
        bg("Outbox feed, request after", {
            let self1 = self.clone();
            async move {
                let txn =
                    self1
                        .0
                        .state
                        .0
                        .db
                        .transaction_on_multi_with_mode(&[TABLE_OUTBOX], web_sys::IdbTransactionMode::Readonly)
                        .context("Failed to start transaction")?;
                let outbox = txn.object_store(TABLE_OUTBOX).context("Failed to get outbox")?;

                // Get entries
                let mut late_stop = true;
                let mut after: Vec<OutboxEntry> = vec![];

                bb!{
                    'read_done _;
                    let Some(
                        cursor
                    ) = outbox.open_cursor_with_range_and_direction(
                        &IdbKeyRange::lower_bound_with_open(
                            &outbox_key(&enum_unwrap!(time.id, FeedId:: Local(_, id) => id)),
                            true,
                        ).unwrap(),
                        IdbCursorDirection::Next
                    ).context("Failed to open outbox cursor") ?.await.context("Error waiting for cursor") ? else {
                        break 'read_done;
                    };
                    loop {
                        if after.len() >= count + 1 {
                            late_stop = false;
                            break 'read_done;
                        }
                        if !cursor
                            .continue_cursor()
                            .context("Error moving cursor forward")?
                            .await
                            .context("Error retrieving cursor advance result")? {
                            break 'read_done;
                        }
                        after.push(from_outbox(&cursor.value()));
                    }
                }

                // Finish read
                txn.await.into_result().context("Failed to commit transaction")?;

                // Combine and send
                eg.event(|pc| {
                    let mut mut_ = self1.0.mut_.borrow_mut();
                    let Some(parent) = mut_.parent.and_then(|p| p.upgrade()) else {
                        return;
                    };
                    parent.respond_entries_after(&None, &time, finish_entries(pc, after), late_stop);
                });
                return Ok(());
            }
        });
    }
}
