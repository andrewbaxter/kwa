use gloo::{
    events::EventListener,
    utils::{
        window,
        format::JsValueSerdeExt,
        document,
    },
    timers::callback::Interval,
};
use js_sys::{
    JsString,
    Array,
};
use wasm_bindgen::{
    prelude::wasm_bindgen,
    JsValue,
};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Url,
    Blob,
    BlobPropertyBag,
    ServiceWorkerRegistration,
};
use crate::{
    util::MyErrorJsValue,
    world::U2SWPost,
};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen]
    type ImportMeta;
    #[wasm_bindgen(method, getter)]
    fn url(this: &ImportMeta) -> JsString;
    #[wasm_bindgen(js_namespace = import, js_name = meta)]
    static IMPORT_META: ImportMeta;
}

pub async fn install() -> Result<ServiceWorkerRegistration, String> {
    EventListener::new(&window(), "controllerchange", |_| {
        window().location().reload().unwrap();
    }).forget();
    let service_workers = window().navigator().service_worker();
    let data_url =
        Url::create_object_url_with_blob(
            &Blob::new_with_str_sequence_and_options(
                &Array::of1(&JsValue::from(include_str!("serviceworker.js"))),
                BlobPropertyBag::new().type_("text/javascript"),
            ).context("Error creating service worker data url")?,
        ).unwrap();
    JsFuture::from(service_workers.register(data_url.as_str())).await.context("Failed to register service worker")?;
    let reg =
        ServiceWorkerRegistration::from(
            JsFuture::from(service_workers.ready().context("Error getting service worker ready future")?)
                .await
                .context("Error waiting for service worker to become ready")?,
        );
    Interval::new(1000, {
        move || {
            let Some(c) = window().navigator().service_worker().controller() else {
                return;
            };
            if !document().hidden() {
                c.post_message(&<JsValue as JsValueSerdeExt>::from_serde(&U2SWPost::Ping).unwrap()).unwrap();
            }
        }
    }).forget();
    return Ok(reg);
}
