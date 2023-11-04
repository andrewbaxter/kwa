use std::{
    panic,
    rc::{
        Rc,
    },
    cell::{
        RefCell,
    },
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
};

fn main() {
    panic::set_hook(Box::new(console_error_panic_hook::hook));
    let eg = lunk::EventGraph::new();
    eg.event(|_pc| {
        struct DemoFeedShared {
            parent: Option<(WeakInfiniscroll<i32>, FeedId)>,
            late_edge: i32,
        }

        struct DemoEntry {
            feed: Rc<RefCell<DemoFeedShared>>,
            t: i32,
        }

        impl DemoEntry {
            fn new(feed: &Rc<RefCell<DemoFeedShared>>, i: i32) -> Rc<dyn Entry<i32>> {
                return Rc::new(DemoEntry {
                    feed: feed.clone(),
                    t: i,
                });
            }
        }

        impl Entry<i32> for DemoEntry {
            fn create_el(&self) -> El {
                return el("div").classes(&["testing_entry"]).text(&self.t.to_string()).on("click", {
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

            fn time(&self) -> i32 {
                return self.t;
            }
        }

        struct DemoFeed {
            shared: Rc<RefCell<DemoFeedShared>>,
            _generate: Option<Interval>,
        }

        impl DemoFeed {
            fn new(initial_count: i32, generate_interval: Option<u32>) -> Self {
                let shared = Rc::new(RefCell::new(DemoFeedShared {
                    parent: None,
                    late_edge: initial_count,
                }));
                return DemoFeed {
                    shared: shared.clone(),
                    _generate: generate_interval.map(|interval| Interval::new(interval, {
                        let shared = Rc::downgrade(&shared);
                        move || {
                            let parent;
                            let id_in_parent;
                            let count;
                            let early;
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
                                count = (random() * 2.) as i32 + 1;
                                early = shared1.late_edge;
                                shared1.late_edge += count;
                            }
                            for i in early .. early + count {
                                logd!("DEMO notify {}", i);
                                parent.notify_entry_after(id_in_parent, i);
                            }
                        }
                    })),
                };
            }
        }

        impl Feed<i32> for DemoFeed {
            fn set_parent(&self, parent: WeakInfiniscroll<i32>, id_in_parent: usize) {
                self.shared.borrow_mut().parent = Some((parent, id_in_parent));
            }

            fn request_around(&self, pivot: i32, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let stop = self1.late_edge;
                let count = count as i32;
                let early_stop;
                let early;
                if count >= pivot {
                    early = 0;
                    early_stop = true;
                } else {
                    early = pivot - count;
                    early_stop = false;
                }
                let late_stop;
                let late;
                if pivot + count >= stop {
                    late = stop;
                    late_stop = true;
                } else {
                    late = pivot + count;
                    late_stop = false;
                }
                spawn_local({
                    let shared = self.shared.clone();
                    async move {
                        TimeoutFuture::new((5000.) as u32).await;

                        //. TimeoutFuture::new(0).await;
                        parent.add_entries_around_initial(
                            id_in_parent,
                            pivot,
                            (early .. late).map(|i| DemoEntry::new(&shared, i)).collect(),
                            early_stop,
                            late_stop,
                        );
                    }
                });
            }

            fn request_before(&self, pivot: i32, count: usize) {
                let self1 = self.shared.borrow_mut();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let count = count as i32;
                let early_stop;
                let early;
                if count >= pivot {
                    early = 0;
                    early_stop = true;
                } else {
                    early = pivot - count;
                    early_stop = false;
                }
                spawn_local({
                    let shared = self.shared.clone();
                    async move {
                        TimeoutFuture::new((1000. + random() * 1000.) as u32).await;
                        parent.respond_entries_before(
                            id_in_parent,
                            pivot,
                            (early .. pivot).rev().map(|i| DemoEntry::new(&shared, i)).collect(),
                            early_stop,
                        );
                    }
                });
            }

            fn request_after(&self, pivot: i32, count: usize) {
                let self1 = self.shared.borrow();
                let (parent, id_in_parent) = self1.parent.as_ref().unwrap();
                let parent = parent.upgrade().unwrap();
                let id_in_parent = *id_in_parent;
                let stop = self1.late_edge;
                let count = count as i32;
                let late_stop;
                let late;
                let early = pivot + 1;
                if early + count >= stop {
                    late = stop;
                    late_stop = true;
                } else {
                    late = early + count;
                    late_stop = false;
                }
                spawn_local({
                    let shared = self.shared.clone();
                    async move {
                        TimeoutFuture::new((1000. + random() * 1000.) as u32).await;
                        let entries: Vec<Rc<dyn Entry<i32>>> =
                            (early .. late).map(|i| DemoEntry::new(&shared, i)).collect();
                        logd!(
                            "DEMO respond after {} -> {:?}; stop {}",
                            pivot,
                            entries.last().map(|e| e.time()),
                            late_stop
                        );
                        parent.respond_entries_after(id_in_parent, pivot, entries, late_stop);
                    }
                });
            }
        }

        let inf1 = Infiniscroll::new(1000, vec![Box::new(DemoFeed::new(1000, Some(5000)))]);

        //. let inf1 = Infiniscroll::new(1000, vec![Box::new(DemoFeed::new(1000, None))]);
        //. inf1.set_padding_post(100.);
        //. let inf2 = Infiniscroll::new(0, vec![Box::new(DemoFeed::new(10, None))]);
        //. inf2.set_padding_pre(100.);
        //. inf2.set_padding_post(100.);
        //. set_root(vec![hbox().extend(vec![inf1.el(), inf2.el()]).own(|_| (inf1, inf2))]);
        //. set_root(vec![hbox().extend(vec![inf2.el()]).own(|_| (inf2))]);
        set_root(vec![hbox().extend(vec![inf1.el()]).own(|_| (inf1))]);
    });
}
