use chrono::{
    DateTime,
    Utc,
};
use futures::Future;
use gloo::utils::format::JsValueSerdeExt;
use indexed_db_futures::{
    IdbDatabase,
    IdbVersionChangeEvent,
    request::{
        OpenDbRequest,
        IdbOpenDbRequestLike,
    },
    idb_object_store::{
        IdbObjectStore,
    },
    idb_transaction::IdbTransaction,
    IdbKeyPath,
};
use serde::{
    Serialize,
    Deserialize,
};
use wasm_bindgen::JsValue;
use web_sys::IdbTransactionMode;
use crate::{
    util::{
        MyErrorDomException,
    },
    world::{
        ChannelId,
        FeedId,
        MessageId,
    },
};

pub const TABLE_OUTBOX: &'static str = "outbox";
pub const TABLE_OUTBOX_INDEX_SENT: &'static str = "sent";
pub const TABLE_OUTBOX_INDEX_STAMP: &'static str = "stamp";

pub async fn new_db() -> Result<IdbDatabase, String> {
    let mut db_req: OpenDbRequest = IdbDatabase::open_u32("main", 1).context("Error opening database")?;
    db_req.set_on_upgrade_needed(Some(|evt: &IdbVersionChangeEvent| -> Result<(), JsValue> {
        if evt.db().object_store_names().find(|n| n == TABLE_OUTBOX).is_none() {
            let outbox = evt.db().create_object_store(TABLE_OUTBOX)?;
            outbox.create_index(TABLE_OUTBOX_INDEX_STAMP, &IdbKeyPath::str("stamp"))?;
            outbox.create_index(TABLE_OUTBOX_INDEX_SENT, &IdbKeyPath::str("sent"))?;
        }
        Ok(())
    }));
    return Ok(db_req.await.context("Error waiting for database to open")?);
}

#[derive(Serialize, Deserialize)]
pub struct OutboxEntryV1 {
    pub stamp: DateTime<Utc>,
    pub channel: ChannelId,
    pub reply: Option<FeedId>,
    pub local_id: String,
    pub body: String,
    pub resolved_id: Option<MessageId>,
}

#[derive(Serialize, Deserialize)]
pub enum OutboxEntry {
    V1(OutboxEntryV1),
}

#[derive(Serialize, Deserialize)]
struct OutboxEntryInner {
    entry: OutboxEntry,
    sent: Vec<String>,
    stamp: DateTime<Utc>,
}

pub fn from_outbox(e: &JsValue) -> OutboxEntry {
    return JsValueSerdeExt::into_serde::<OutboxEntryInner>(e).unwrap().entry;
}

pub fn outbox_sent_partial_key_unsent() -> JsValue {
    return <JsValue as JsValueSerdeExt>::from_serde(&["0"]).unwrap();
}

pub fn outbox_sent_partial_key_sent() -> JsValue {
    return <JsValue as JsValueSerdeExt>::from_serde(&["1"]).unwrap();
}

fn sent_key(sent: bool) -> &'static str {
    return match sent {
        true => "1",
        false => "0",
    };
}

pub fn outbox_sent_key(local_id: &str, sent: bool) -> JsValue {
    return <JsValue as JsValueSerdeExt>::from_serde(&[sent_key(sent), local_id]).unwrap();
}

pub fn outbox_key(local_id: &str) -> JsValue {
    return <JsValue as JsValueSerdeExt>::from_serde(local_id).unwrap();
}

pub async fn put_outbox<'a>(store: &IdbObjectStore<'a>, e: OutboxEntry) {
    let local_id;
    let resolved;
    let stamp;
    match &e {
        OutboxEntry::V1(e) => {
            local_id = e.local_id.clone();
            resolved = e.resolved_id.is_some();
            stamp = e.stamp.clone();
        },
    };
    store.put_key_val(&outbox_key(&local_id), &<JsValue as JsValueSerdeExt>::from_serde(&OutboxEntryInner {
        entry: e,
        sent: vec![local_id, sent_key(resolved).to_string()],
        stamp: stamp,
    }).unwrap()).unwrap().await.unwrap();
}
