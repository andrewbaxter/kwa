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
use lunk::{
    ProcessingContext,
    EventGraph,
};
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
        struct DemoId(i64, &'static str);

        struct DemoFeedMut {
            parent: Option<WeakInfiniscroll<i32, DemoId>>,
            start: i64,
            hist: Vec<i64>,
            _generate: Option<Interval>,
        }

        impl DemoFeedMut {
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
            t: DemoId,
            feed: DemoFeed,
        }

        impl DemoEntry {
            fn new(feed: &DemoFeed, i: DemoId) -> Rc<dyn Entry<DemoId>> {
                return Rc::new(DemoEntry {
                    feed: feed.clone(),
                    t: i,
                });
            }
        }

        impl Entry<DemoId> for DemoEntry {
            fn create_el(&self, pc: &mut ProcessingContext) -> El {
                return el("div")
                    .classes(&["testing_entry"])
                    .text(&format!("{} {}", self.t.1, self.t.0 as f64 / 1000.))
                    .on("click", {
                        let feed = self.feed.clone();
                        let mut selected = false;
                        let t = self.t;
                        move |e| {
                            logn!("clickoo");
                            let Some(parent) =
                            //. .
                            & feed.0.mut_.borrow().parent else {
                                return;
                            };
                            let Some(parent) = parent.upgrade() else {
                                return;
                            };
                            let e = el_from_raw(e.target().unwrap().dyn_into().unwrap());
                            selected = !selected;
                            if selected {
                                e.ref_classes(&["sticky"]);
                                parent.set_sticky(&t);
                            } else {
                                e.ref_remove_classes(&["sticky"]);
                                parent.clear_sticky();
                            }
                        }
                    });
            }

            fn time(&self) -> DemoId {
                return self.t;
            }
        }

        struct DemoFeed_ {
            id: i32,
            name: &'static str,
            mut_: RefCell<DemoFeedMut>,
        }

        #[derive(Clone)]
        struct DemoFeed(Rc<DemoFeed_>);

        impl DemoFeed {
            fn new(
                eg: &EventGraph,
                id: i32,
                name: &'static str,
                initial_count: usize,
                generate_interval: Option<u32>,
            ) -> Self {
                let start = Utc::now().timestamp_millis();
                let mut hist = vec![];
                for i in 0 .. initial_count {
                    hist.push((-(initial_count as i64) + i as i64) * 1000);
                }
                let out = DemoFeed(Rc::new(DemoFeed_ {
                    id: id,
                    name: name,
                    mut_: RefCell::new(DemoFeedMut {
                        parent: None,
                        start: start,
                        hist: hist,
                        _generate: None,
                    }),
                }));
                out.0.mut_.borrow_mut()._generate = generate_interval.map(|interval| Interval::new(interval, {
                    let self1 = Rc::downgrade(&out.0);
                    let eg = eg.clone();
                    move || {
                        let Some(self1) = self1.upgrade() else {
                            return;
                        };
                        let self1 = DemoFeed(self1);
                        let parent;
                        let i;
                        {
                            let mut mut_ = self1.0.mut_.borrow_mut();
                            let mut_ = &mut *mut_;
                            let Some(parent0) =
                            //. .
                            & mut_.parent else {
                                return;
                            };
                            let Some(parent0) = parent0.upgrade() else {
                                return;
                            };
                            parent = parent0;
                            let time = Utc::now().timestamp_millis() - mut_.start;
                            mut_.hist.push(time);
                            i = DemoId(time, self1.0.name);
                        }
                        if let Some((pivot, count)) = parent.want_after(id, i) {
                            self1.request_after(eg.clone(), pivot, count);
                        }
                    }
                }));
                return out;
            }
        }

        impl Feed<i32, DemoId> for DemoFeed {
            fn set_parent(&self, parent: WeakInfiniscroll<i32, DemoId>) {
                self.0.mut_.borrow_mut().parent = Some(parent);
            }

            fn request_around(&self, _eg: EventGraph, pivot: DemoId, count: usize) {
                let mut_ = self.0.mut_.borrow();
                let parent = mut_.parent.as_ref().unwrap().upgrade().unwrap();
                let i = mut_.find(pivot.0);
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
                        if i + count >= mut_.hist.len() {
                            late_stop = true;
                            late = mut_.hist.len();
                        } else {
                            late_stop = false;
                            late = i + count;
                        }
                        out = mut_.hist[early .. late].to_vec();
                    },
                    None => {
                        early_stop = true;
                        late_stop = true;
                        out = vec![];
                    },
                }
                spawn_local({
                    let self1 = self.clone();
                    async move {
                        TimeoutFuture::new((random() * 1000. + 4000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.respond_entries_around(
                            self1.0.id,
                            pivot,
                            out.into_iter().map(|t| DemoEntry::new(&self1, DemoId(t, self1.0.name))).collect(),
                            early_stop,
                            late_stop,
                        );
                    }
                });
            }

            fn request_before(&self, _eg: EventGraph, pivot: DemoId, count: usize) {
                let mut_ = self.0.mut_.borrow();
                let parent = mut_.parent.as_ref().unwrap().upgrade().unwrap();
                let i = mut_.find(pivot.0);
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
                        out = mut_.hist[early .. i].to_vec();
                    },
                    None => {
                        early_stop = true;
                        out = vec![];
                    },
                }
                spawn_local({
                    let self1 = self.clone();
                    async move {
                        TimeoutFuture::new((random() * 1000. + 4000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.respond_entries_before(
                            &self1.0.id,
                            &pivot,
                            out
                                .into_iter()
                                .rev()
                                .map(|t| DemoEntry::new(&self1, DemoId(t, self1.0.name)))
                                .collect(),
                            early_stop,
                        );
                    }
                });
            }

            fn request_after(&self, _eg: EventGraph, pivot: DemoId, count: usize) {
                let mut_ = self.0.mut_.borrow();
                let parent = mut_.parent.as_ref().unwrap().upgrade().unwrap();
                let i = mut_.find(pivot.0);
                let out;
                let late_stop;
                match i {
                    Some(i) => {
                        let late;
                        if i + count >= mut_.hist.len() {
                            late_stop = true;
                            late = mut_.hist.len();
                        } else {
                            late_stop = false;
                            late = i + count;
                        }
                        out = mut_.hist[i + 1 .. late].to_vec();
                    },
                    None => {
                        late_stop = true;
                        out = vec![];
                    },
                }
                logd!("request after, out {:?}", out);
                spawn_local({
                    let self1 = self.clone();
                    async move {
                        TimeoutFuture::new((random() * 1000. + 4000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.respond_entries_after(
                            &self1.0.id,
                            &pivot,
                            out.into_iter().map(|t| DemoEntry::new(&self1, DemoId(t, self1.0.name))).collect(),
                            late_stop,
                        );
                    }
                });
            }
        }

        let now = 1000;

        //. let inf1 = Infiniscroll::new(now, vec![Box::new(DemoFeed::new("alpha", 1000, Some(5000)))]);
        let eg = EventGraph::new();
        let inf1 =
            Infiniscroll::new(
                &eg,
                DemoId(now, "alpha"),
                [
                    Box::new(DemoFeed::new(&eg, 0, "alpha", 1000, Some(5000))),
                    Box::new(DemoFeed::new(&eg, 1, "beta", 500, Some(4500))),
                ]
                    .into_iter()
                    .map(|f| (f.0.id, f as Box<dyn Feed<i32, DemoId>>))
                    .collect(),
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
