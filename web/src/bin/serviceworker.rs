use wasm_bindgen::{
    JsCast,
};
use web::{
    NOTIFY_CHANNEL,
    world::{
        S2SWPush,
        DateMessageId,
    },
    util::{
        MyErrorJsValue,
    },
};
use web_sys::{
    BroadcastChannel,
    PushEvent,
    ServiceWorkerGlobalScope,
    NotificationOptions,
    ExtendableEvent,
};

fn main() {
    let global = js_sys::global().unchecked_into::<ServiceWorkerGlobalScope>();
    gloo::events::EventListener::new(&global, "install", {
        let global = global.clone();
        move |_| {
            _ = global.skip_waiting().unwrap();
        }
    }).forget();
    gloo::events::EventListener::new(&global, "activate", {
        let global = global.clone();
        move |e| {
            let e = e.dyn_ref::<ExtendableEvent>().unwrap();
            e.wait_until(&global.clients().claim()).unwrap();
        }
    }).forget();
    gloo::events::EventListener::new(&global, "push", {
        let bc = BroadcastChannel::new(NOTIFY_CHANNEL).unwrap();
        let global = global.clone();
        move |e| {
            let e = e.dyn_ref::<PushEvent>().unwrap();
            let body = serde_json::from_str::<S2SWPush>(&e.data().unwrap().text()).unwrap();
            bc.post_message(&serde_json::to_string(&DateMessageId(body.time, body.id)).unwrap().into()).unwrap();
            match global.registration().show_notification_with_options(&body.title, &{
                let mut o = NotificationOptions::new();
                o.body(&body.quote);
                o.icon(&body.icon_url);
                o
            }) {
                Ok(p) => {
                    e.wait_until(&p).log_ignore("Failed to wait for notification promise");
                },
                Err(e) => {
                    Err::<(), _>(e).log_ignore("Failed to create notification");
                },
            }
        }
    }).forget();
}
