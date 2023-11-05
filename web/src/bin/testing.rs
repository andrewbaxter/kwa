use std::{
    panic,
    rc::{
        Rc,
    },
    cell::{
        RefCell,
    },
};
use chrono::{
    DateTime,
    Utc,
    NaiveDateTime,
};
use gloo::timers::{
    callback::{
        Interval,
    },
    future::TimeoutFuture,
};
use js_sys::Math::random;
use rooting::{
    set_root,
    el,
    El,
    el_from_raw,
};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web::{
    infiniscroll::{
        Entry,
        FeedId,
        WeakInfiniscroll,
        Feed,
        Infiniscroll,
    },
    html::hbox,
    logn,
    logd,
    bb,
};

fn main() {
    panic::set_hook(Box::new(console_error_panic_hook::hook));
    let eg = lunk::EventGraph::new();
    eg.event(|_pc| {
        #[derive(Clone, Copy, Eq, PartialEq, PartialOrd, Debug, Hash)]
        struct DemoFeedId(i64, &'static str);

        struct DemoFeedShared {
            parent: Option<(WeakInfiniscroll<DemoFeedId>, FeedId)>,
            name: &'static str,
            start: i64,
            hist: Vec<i64>,
        }

        impl DemoFeedShared {
            fn find(&self, pivot: i64) -> Option<usize> {
                let mut last = None;
                for (i, e) in self.hist.iter().enumerate() {
                    if *e > pivot {
                        break;
                    }
                    last = Some(i);
                }
                return last;
            }
        }

        struct DemoEntry {
            feed: Rc<RefCell<DemoFeedShared>>,
            t: DemoFeedId,
        }

        impl DemoEntry {
            fn new(feed: &Rc<RefCell<DemoFeedShared>>, i: DemoFeedId) -> Rc<dyn Entry<DemoFeedId>> {
                return Rc::new(DemoEntry {
                    feed: feed.clone(),
                    t: i,
                });
            }
        }

        impl Entry<DemoFeedId> for DemoEntry {
            fn create_el(&self) -> El {
                return el("div")
                    .classes(&["testing_entry"])
                    .text(&format!("{} {}", self.t.1, self.t.0 as f64 / 1000.))
                    .on("click", {
                        let feed = self.feed.clone();
                        let mut selected = false;
                        let t = self.t;
                        move |e| {
                            logn!("clickoo");
                            let Some((parent, id_in_parent)) =
                            //. .
                            & feed.borrow().parent else {
                                return;
                            };
                            let Some(parent) = parent.upgrade() else {
                                return;
                            };
                            let e = el_from_raw(e.target().unwrap().dyn_into().unwrap());
                            selected = !selected;
                            if selected {
                                e.ref_classes(&["sticky"]);
                                parent.sticky(*id_in_parent, t);
                            } else {
                                e.ref_remove_classes(&["sticky"]);
                                parent.unsticky(t);
                            }
                        }
                    });
            }

            fn time(&self) -> DemoFeedId {
                return self.t;
            }
        }

        struct DemoFeed {
            shared: Rc<RefCell<DemoFeedShared>>,
            _generate: Option<Interval>,
        }

        impl DemoFeed {
            fn new(name: &'static str, initial_count: usize, generate_interval: Option<u32>) -> Self {
                let start = Utc::now().timestamp_millis();
                let mut hist = vec![];
                for i in 0 .. initial_count {
                    hist.push((-(initial_count as i64) + i as i64) * 1000);
                }
                let shared = Rc::new(RefCell::new(DemoFeedShared {
                    parent: None,
                    name: name,
                    start: start,
                    hist: hist,
                }));
                return DemoFeed {
                    shared: shared.clone(),
                    _generate: generate_interval.map(|interval| Interval::new(interval, {
                        let shared = Rc::downgrade(&shared);
                        move || {
                            let parent;
                            let id_in_parent;
                            let i;
                            {
                                let Some(shared) = shared.upgrade() else {
                                    return;
                                };
                                let mut shared1 = shared.borrow_mut();
                                let shared1 = &mut *shared1;
                                let Some((parent0, id_in_parent0)) =
                                //. .
                                & shared1.parent else {
                                    return;
                                };
                                id_in_parent = *id_in_parent0;
                                let Some(parent0) = parent0.upgrade() else {
                                    return;
                                };
                                parent = parent0;
                                let time = Utc::now().timestamp_millis() - shared1.start;
                                shared1.hist.push(time);
                                i = DemoFeedId(time, shared1.name);
                            }
                            parent.notify_entry_after(id_in_parent, i);
                        }
                    })),
                };
            }
        }

        impl Feed<DemoFeedId> for DemoFeed {
            fn set_parent(&self, parent: WeakInfiniscroll<DemoFeedId>, id_in_parent: usize) {
                self.shared.borrow_mut().parent = Some((parent, id_in_parent));
            }

            fn request_around(&self, pivot: DemoFeedId, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let i = self1.find(pivot.0);
                let name = self1.name;
                let out;
                let early_stop;
                let late_stop;
                match i {
                    Some(i) => {
                        let early;
                        let late;
                        if i <= count {
                            early_stop = true;
                            early = 0;
                        } else {
                            early_stop = false;
                            early = i - count;
                        }
                        if i + count >= self1.hist.len() {
                            late_stop = true;
                            late = self1.hist.len();
                        } else {
                            late_stop = false;
                            late = i + count;
                        }
                        out = self1.hist[early .. late].to_vec();
                    },
                    None => {
                        early_stop = true;
                        late_stop = true;
                        out = vec![];
                    },
                }
                spawn_local({
                    let shared = self.shared.clone();
                    async move {
                        TimeoutFuture::new((random() * 1000. + 4000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.respond_entries_around(
                            id_in_parent,
                            pivot,
                            out.into_iter().map(|t| DemoEntry::new(&shared, DemoFeedId(t, name))).collect(),
                            early_stop,
                            late_stop,
                        );
                    }
                });
            }

            fn request_before(&self, pivot: DemoFeedId, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let i = self1.find(pivot.0);
                let name = self1.name;
                let out;
                let early_stop;
                match i {
                    Some(i) => {
                        let early;
                        if i <= count {
                            early_stop = true;
                            early = 0;
                        } else {
                            early_stop = false;
                            early = i - count;
                        }
                        out = self1.hist[early .. i].to_vec();
                    },
                    None => {
                        early_stop = true;
                        out = vec![];
                    },
                }
                spawn_local({
                    let shared = self.shared.clone();
                    async move {
                        TimeoutFuture::new((random() * 1000. + 4000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.respond_entries_before(
                            id_in_parent,
                            pivot,
                            out.into_iter().rev().map(|t| DemoEntry::new(&shared, DemoFeedId(t, name))).collect(),
                            early_stop,
                        );
                    }
                });
            }

            fn request_after(&self, pivot: DemoFeedId, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let i = self1.find(pivot.0);
                let name = self1.name;
                let out;
                let late_stop;
                match i {
                    Some(i) => {
                        let late;
                        if i + count >= self1.hist.len() {
                            late_stop = true;
                            late = self1.hist.len();
                        } else {
                            late_stop = false;
                            late = i + count;
                        }
                        out = self1.hist[i + 1 .. late].to_vec();
                    },
                    None => {
                        late_stop = true;
                        out = vec![];
                    },
                }
                logd!("request after, out {:?}", out);
                spawn_local({
                    let shared = self.shared.clone();
                    async move {
                        TimeoutFuture::new((random() * 1000. + 4000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.respond_entries_after(
                            id_in_parent,
                            pivot,
                            out.into_iter().map(|t| DemoEntry::new(&shared, DemoFeedId(t, name))).collect(),
                            late_stop,
                        );
                    }
                });
            }
        }

        let now = 1000;

        //. let inf1 = Infiniscroll::new(now, vec![Box::new(DemoFeed::new("alpha", 1000, Some(5000)))]);
        let inf1 =
            Infiniscroll::new(
                DemoFeedId(now, "alpha"),
                vec![
                    Box::new(DemoFeed::new("alpha", 1000, Some(5000))),
                    Box::new(DemoFeed::new("beta", 500, Some(4500)))
                ],
            );

        //. let inf1 = Infiniscroll::new(1000, vec![Box::new(DemoFeed::new(1000, None))]);
        //. inf1.set_padding_post(100.);
        //. let inf2 = Infiniscroll::new(0, vec![Box::new(DemoFeed::new(10, None))]);
        //. inf2.set_padding_pre(100.);
        //. inf2.set_padding_post(100.);
        //. set_root(vec![hbox().extend(vec![inf1.el(), inf2.el()]).own(|_| (inf1, inf2))]);
        //. set_root(vec![hbox().extend(vec![inf2.el()]).own(|_| (inf2))]);
        //. set_root(vec![hbox().extend(vec![inf1.el()]).own(|_| (inf1))]);
        set_root(vec![hbox().extend(vec![inf1.el()]).own(|_| (inf1))]);
    });
}
