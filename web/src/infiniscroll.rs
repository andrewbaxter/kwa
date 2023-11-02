//! # Control
//!
//! The infiniscroll deals with lots of shakey parts by maintaining a limited set
//! of "desired" state, and the rest is synced to that:
//!
//! * Anchor element - which element is the screen "anchor".
//!
//! * Anchor element offset - how the origin is offset from the anchor (fine scrolling,
//!   within a single element)
//!
//! * Origin alignment - how the screen relates to the anchor element.  0 means the top
//!   of the anchor is at the top of the screen, 0.5 means the middle of the anchor is
//!   in the middle of the screen, 1 means the bottom of the anchor is at the bottom of
//!   the screen.
//!
//! When the user scrolls the anchor element and offset change.
//!
//! When the user scrolls to the end/start, or the stop element is reached via
//! pulling, the alignment is changed.
//!
//! Scrolling, resizing, etc. subsequently "shake" everything to remove wrinkles,
//! matching everything with the above values, triggering new entry requests, etc.
//!
//! # Shake
//!
//! Shake has two parts
//!
//! 1. Coming up with logical element layout
//!
//! 2. Matching the view to that layout
//!
//! # Sticky
//!
//! When realized, a sticky element is like normal, relying on css to keep it on
//! screen.
//!
//! When it moves into reserve, the realized state is moved into an early/late feed
//! holding bucket, and the dom element remains in tree.  It stays even if the
//! reserve is dropped.
//!
//! When the reserve is consumed, if the next item is sticky, it's just moved out
//! of the sticky bucket.
use std::{
    rc::{
        Rc,
        Weak,
    },
    cell::{
        RefCell,
        Cell,
    },
    collections::{
        HashMap,
        VecDeque,
        HashSet,
    },
    hash::Hash,
};
use chrono::{
    Utc,
    DateTime,
    Duration,
};
use gloo::{
    timers::callback::{
        Timeout,
    },
};
use rooting::{
    el,
    El,
    ResizeObserver,
    ObserveHandle,
    ContainerEntry,
    Container,
};
use wasm_bindgen::{
    JsCast,
};
use web_sys::HtmlElement;
use crate::{
    bb,
    logd,
    html::{
        stack,
    },
    util::MoreMath,
};

const PX_PER_CM: f64 = 96. / 2.54;
const BUFFER: f64 = PX_PER_CM * 40.;
const CSS_HIDE: &'static str = "hide";
const REQUEST_COUNT: usize = 50;
const MIN_RESERVE: usize = 50;
const MAX_RESERVE: usize = MIN_RESERVE + 2 * REQUEST_COUNT;
pub type FeedId = usize;

trait ElExt {
    fn offset_top(&self) -> f64;
    fn offset_height(&self) -> f64;
}

impl ElExt for El {
    fn offset_top(&self) -> f64 {
        return self.raw().dyn_ref::<HtmlElement>().unwrap().offset_top() as f64;
    }

    fn offset_height(&self) -> f64 {
        return self.raw().dyn_ref::<HtmlElement>().unwrap().offset_height() as f64;
    }
}

pub trait IdTraits: Clone + std::fmt::Debug + PartialEq + Eq + PartialOrd + Hash { }

impl<T: Clone + std::fmt::Debug + PartialEq + Eq + PartialOrd + Hash> IdTraits for T { }

/// Represents an atom in the infinite scroller.
pub trait Entry<Id> {
    fn create_el(&self) -> El;
    fn time(&self) -> Id;
}

struct EntryState<Id> {
    feed_id: FeedId,
    entry: Rc<dyn Entry<Id>>,
    entry_el: El,
    _entry_el_observe: ObserveHandle,
}

impl<Id> ContainerEntry for EntryState<Id> {
    fn el(&self) -> &El {
        return &self.entry_el;
    }
}

/// A data source for the inifiniscroller. When it gets requests for elements, it
/// must only call the parent `add` functions after the stack unwinds (spawn or
/// timer next tick).
///
/// The stop states of the feed are controlled by the feed when it calls `add`
/// methods.  `add_entry_after_stop` will be discarded if not in the stop state but
/// if it is in the stop state, the element added must be the element immediately
/// after the last `add_entry_after_nostop` entry when the stop state was entered,
/// or immediately following a previous entry added with `add_entry_after_stop`.
pub trait Feed<Id: IdTraits> {
    fn set_parent(&self, parent: WeakInfiniscroll<Id>, id_in_parent: FeedId);
    fn request_around(&self, time: Id, count: usize);
    fn request_before(&self, time: Id, count: usize);
    fn request_after(&self, time: Id, count: usize);
}

struct FeedState<Id> {
    feed: Box<dyn Feed<Id>>,
    /// No elements, shortcut for request_around for initial data
    initial: bool,
    /// All entries are sorted and come before all realized entries. Front = nearest to
    /// real = late to early.
    early_reserve: VecDeque<Rc<dyn Entry<Id>>>,
    /// All entries are sorted and come after all realized entries. Front = nearest to
    /// real = early to late.
    late_reserve: VecDeque<Rc<dyn Entry<Id>>>,
    early_stop: bool,
    late_stop: bool,
}

struct Infiniscroll_<Id: Clone + Hash + PartialEq> {
    /// Used when new/resetting
    reset_time: Id,
    frame: El,
    cached_frame_height: f64,
    content: El,
    content_layout: El,
    /// Mirrors content's height, used to avoid js round trips (keep in sync)
    logical_content_height: f64,
    logical_content_layout_offset: f64,
    logical_scroll_top: f64,
    center_spinner: El,
    early_spinner: El,
    late_spinner: El,
    feeds: HashMap<FeedId, FeedState<Id>>,
    sticky_set: HashSet<Id>,
    /// Duplicate elements in early reserve, where the sticky element is still part of
    /// the dom.  When an element is put in real, remove from here. However a discarded
    /// sticky reserve element stays here.
    ///
    /// Back = nearest to real.
    early_sticky: Container<EntryState<Id>>,
    /// Duplicate elements in late reserve, where the sticky element is still part of
    /// the dom.  When an element is put in real, remove from here. However a discarded
    /// sticky reserve element stays here.
    ///
    /// Back = nearest to real.
    late_sticky: Container<EntryState<Id>>,
    /// All entries are sorted.
    real: Container<EntryState<Id>>,
    cached_real_offset: f64,
    /// None if real is empty (i.e. invalid index)
    anchor_i: Option<usize>,
    anchor_alignment: f64,
    /// Offset of anchor element origin from view (scrolling)/desired content
    /// (recentering) origin.  If alignment is 0 (origin is top of element), has range
    /// `-height..0` because if the element is below the origin the anchor would
    /// actually be the previous element. If alignment is 1, has range `0..height`.
    anchor_offset: f64,
    delay_update: Option<Timeout>,
    delay_update_last: DateTime<Utc>,
    entry_resize_observer: Option<ResizeObserver>,
    // Ignore scroll events until this time, because they're probably relayout
    // generated and not human
    mute_scroll: DateTime<Utc>,
}

impl<Id: IdTraits> Infiniscroll_<Id> {
    fn update_logical_scroll(&mut self, want_early: f64, want_late: f64) {
        let lower = want_early;
        let upper = self.logical_content_height - want_late - self.cached_frame_height;
        self.logical_scroll_top = (lower + (upper - lower) * self.anchor_alignment).max(0.);
        logd!(
            "update logical scroll: {} - {} / {}; origin y is {}",
            self.logical_scroll_top,
            self.logical_scroll_top + self.cached_frame_height,
            self.logical_content_height,
            self.logical_scroll_top + self.cached_frame_height * self.anchor_alignment
        );
        if let Some(anchor_i) = &self.anchor_i {
            logd!("update logical scroll: post anchor origin {}", {
                let base_e_content_offset = self.logical_content_layout_offset + self.cached_real_offset;
                let anchor = self.real.get(*anchor_i).unwrap();
                base_e_content_offset + anchor.entry_el.offset_top() +
                    anchor.entry_el.offset_height() * self.anchor_alignment -
                    self.anchor_offset
            });
        }
    }

    fn reanchor_inner(&mut self, mut anchor_i: usize, content_origin_y: f64) {
        // Move anchor pointer down until directly after desired element
        while let Some(e_state) = self.real.get(anchor_i + 1) {
            if e_state.entry_el.offset_top() > content_origin_y {
                break;
            }
            logd!(
                "move anchor_i +1: {} = {} > {}",
                e_state.entry_el.offset_top(),
                e_state.entry_el.offset_top(),
                content_origin_y
            );
            anchor_i += 1;
        }

        // Move anchor pointer up until directly above (=at) desired element.
        while let Some(e_state) = self.real.get(anchor_i) {
            if e_state.entry_el.offset_top() <= content_origin_y {
                break;
            }
            if anchor_i == 0 {
                break;
            }
            logd!(
                "move anchor_i -1: {} = {} > {}",
                e_state.entry_el.offset_top(),
                e_state.entry_el.offset_top(),
                content_origin_y
            );
            anchor_i -= 1;
        }

        // Calculate offset
        let anchor = self.real.get(anchor_i).unwrap();
        let anchor_height = anchor.entry_el.offset_height();
        let anchor_top = anchor.entry_el.offset_top();
        let anchor_origin_y = anchor_top + anchor_height * self.anchor_alignment;
        self.anchor_offset =
            (anchor_origin_y -
                content_origin_y).clamp(
                -anchor_height + anchor_height * self.anchor_alignment,
                anchor_height * self.anchor_alignment,
            );

        // .
        self.anchor_i = Some(anchor_i);
    }

    fn scroll_reanchor(&mut self) {
        let old_anchor_i = self.anchor_i;
        let old_anchor_offset = self.anchor_offset;
        if let Some(anchor_i) = self.anchor_i {
            let content_origin_y = 
                // Origin in content space
                self.logical_scroll_top + self.cached_frame_height * self.anchor_alignment
                // Origin in content-layout space
                - self.logical_content_layout_offset - self.cached_real_offset;
            self.reanchor_inner(anchor_i, content_origin_y);
        } else {
            self.anchor_i = None;
            self.anchor_offset = 0.;
        }
        logd!("Reanchor {:?} {} to {:?} {}", old_anchor_i, old_anchor_offset, self.anchor_i, self.anchor_offset);
    }

    // Change anchor based on logical values (anchor, alignment), + frame height
    fn transition_alignment_reanchor(&mut self) {
        let Some(anchor_i) = self.anchor_i.clone() else {
            return;
        };
        let anchor = self.real.get(anchor_i).unwrap();
        let origin_y =
            anchor.entry_el.offset_top() + anchor.entry_el.offset_height() * self.anchor_alignment -
                self.anchor_offset;
        logd!(
            "transition: origin y = {} + {} * {} - {} = {}",
            anchor.entry_el.offset_top(),
            anchor.entry_el.offset_height(),
            self.anchor_alignment,
            self.anchor_offset,
            anchor.entry_el.offset_top() + anchor.entry_el.offset_height() * self.anchor_alignment - self.anchor_offset
        );
        let frame_early = origin_y - self.cached_frame_height * self.anchor_alignment;
        let frame_late = origin_y + self.cached_frame_height * (1. - self.anchor_alignment);
        let old_anchor_alignment = self.anchor_alignment;
        let mut early_all_end = true;
        let mut late_all_end = true;
        for f in self.feeds.values() {
            early_all_end = early_all_end && f.early_stop && f.early_reserve.is_empty();
            late_all_end = late_all_end && f.late_stop && f.late_reserve.is_empty();
        }
        let last_el = self.real.last().unwrap();
        let last_el_top = last_el.entry_el.offset_top();
        let first_el = self.real.first().unwrap();
        let first_el_bottom = first_el.entry_el.offset_height();
        logd!(
            "anchor {} / {}; origin y {}; set stops, early end {}, late end {}; frame early {}, late {}; first el bottom {}; last el top {}",
            anchor_i,
            self.real.len(),
            origin_y,
            early_all_end,
            late_all_end,
            frame_early,
            frame_late,
            first_el_bottom,
            last_el_top
        );

        // # Hovering late end, align to late end
        if late_all_end && frame_late >= last_el_top {
            self.anchor_alignment = 1.;
            logd!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
            self.anchor_i = Some(self.real.len() - 1);
            self.anchor_offset = frame_late.min(last_el_top) - last_el_top;
            return;
        }

        // # Hovering early end, align to early end
        if early_all_end && frame_early <= first_el_bottom {
            self.anchor_alignment = 0.;
            logd!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
            self.anchor_i = Some(0);
            self.anchor_offset = frame_early.max(0.);
            return;
        }

        // # Otherwise, revert to middle
        self.anchor_alignment = 0.5;
        logd!("Set alignment {} -> {}", old_anchor_alignment, self.anchor_alignment);
        let new_origin_y = (frame_early + frame_late) / 2.;
        self.reanchor_inner(anchor_i, new_origin_y);
    }
}

fn get_pivot_early<
    Id: Clone,
>(entries: &Container<EntryState<Id>>, feed_id: FeedId, f_state: &FeedState<Id>) -> Option<Id> {
    return f_state
        .early_reserve
        .back()
        .map(|entry| entry.time())
        .or_else(
            || entries.iter().filter(|entry| entry.feed_id == feed_id).map(|e_state| e_state.entry.time()).next(),
        )
        .or_else(|| f_state.late_reserve.front().map(|entry| entry.time()));
}

fn get_pivot_late<
    Id: Clone,
>(entries: &Container<EntryState<Id>>, feed_id: FeedId, f_state: &FeedState<Id>) -> Option<Id> {
    return f_state
        .late_reserve
        .back()
        .map(|entry| entry.time())
        .or_else(
            || entries
                .iter()
                .rev()
                .filter(|entry| entry.feed_id == feed_id)
                .map(|e_state| e_state.entry.time())
                .next(),
        )
        .or_else(|| f_state.early_reserve.front().map(|entry| entry.time()));
}

fn realize_entry<
    Id: Clone,
>(entry_resize_observer: &ResizeObserver, feed_id: FeedId, entry: Rc<dyn Entry<Id>>) -> EntryState<Id> {
    let entry_el = entry.create_el();
    return EntryState {
        feed_id: feed_id,
        entry: entry,
        entry_el: entry_el.clone(),
        _entry_el_observe: entry_resize_observer.observe(&entry_el),
    };
}

#[derive(Clone)]
pub struct WeakInfiniscroll<Id: IdTraits>(Weak<RefCell<Infiniscroll_<Id>>>);

impl<Id: IdTraits> WeakInfiniscroll<Id> {
    pub fn upgrade(&self) -> Option<Infiniscroll<Id>> {
        return self.0.upgrade().map(Infiniscroll);
    }
}

#[derive(Clone)]
pub struct Infiniscroll<Id: IdTraits>(Rc<RefCell<Infiniscroll_<Id>>>);

impl<Id: IdTraits + 'static> Infiniscroll<Id> {
    pub fn new(reset_id: Id, feeds: Vec<Box<dyn Feed<Id>>>) -> Self {
        let outer_stack = stack();
        let frame = el("div").classes(&["infinite"]);
        let content = el("div").classes(&["content"]);
        let content_layout = el("div").classes(&["content_layout"]);
        let content_lines_early_sticky = Container::new(el("div").classes(&["sticky"]));
        let content_lines_real = Container::new(el("div").classes(&["real"]));
        let content_lines_late_sticky = Container::new(el("div").classes(&["sticky"]));
        let center_spinner = el("div").classes(&["center_spinner"]);
        let early_spinner = el("div").classes(&["early_spinner"]);
        let late_spinner = el("div").classes(&["late_spinner"]);
        outer_stack.ref_extend(vec![frame.clone(), center_spinner.clone()]);
        frame.ref_push(content.clone());
        content.ref_push(content_layout.clone());
        content_layout.ref_extend(
            vec![
                early_spinner.clone(),
                content_lines_early_sticky.el().clone(),
                content_lines_real.el().clone(),
                content_lines_late_sticky.el().clone(),
                late_spinner.clone()
            ],
        );
        let state = Infiniscroll(Rc::new(RefCell::new(Infiniscroll_ {
            reset_time: reset_id,
            frame: frame.clone(),
            cached_frame_height: 0.,
            content: content.clone(),
            content_layout: content_layout,
            logical_content_height: 0.,
            logical_content_layout_offset: 0.,
            logical_scroll_top: 0.,
            center_spinner: center_spinner,
            early_spinner: early_spinner,
            late_spinner: late_spinner,
            feeds: HashMap::new(),
            sticky_set: HashSet::new(),
            early_sticky: content_lines_early_sticky,
            real: content_lines_real,
            cached_real_offset: 0.,
            late_sticky: content_lines_late_sticky,
            anchor_i: None,
            anchor_alignment: 0.5,
            anchor_offset: 0.,
            delay_update: None,
            delay_update_last: DateTime::<Utc>::MIN_UTC,
            entry_resize_observer: None,
            mute_scroll: Utc::now() + Duration::milliseconds(300),
        })));
        let entry_resize_observer = Some(ResizeObserver::new({
            let state = state.weak();
            move |_| {
                logd!("resize cb on real");
                let Some(state) = state.upgrade() else {
                    return;
                };
                //. .state.shake();
            }
        }));
        {
            let mut state1 = state.0.borrow_mut();
            let weak_state = state.weak();
            for (i, feed) in feeds.into_iter().enumerate() {
                feed.set_parent(weak_state.clone(), i);
                state1.feeds.insert(i, FeedState {
                    feed: feed,
                    initial: true,
                    early_reserve: VecDeque::new(),
                    late_reserve: VecDeque::new(),
                    early_stop: false,
                    late_stop: false,
                });
            }
            state1.entry_resize_observer = entry_resize_observer;
        }
        frame.ref_on("scroll", {
            let state = state.weak();
            move |_event| {
                logd!("EV scroll");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    if state1.mute_scroll >= Utc::now() {
                        return;
                    }
                    state1.logical_scroll_top = state1.frame.raw().scroll_top() as f64;
                    state1.scroll_reanchor();
                    state1.transition_alignment_reanchor();
                }
                state.shake();
            }
        });
        frame.ref_on_resize({
            // Frame height change
            let state = state.weak();
            move |_, _, frame_height| {
                logd!("EV frame resize");
                let Some(state) = state.upgrade() else {
                    return;
                };
                {
                    let mut state1 = state.0.borrow_mut();
                    if frame_height == state1.cached_frame_height {
                        return;
                    }
                    state1.cached_frame_height = frame_height;
                    state1.mute_scroll = Utc::now() + Duration::milliseconds(50);
                }
                state.shake();
            }
        });
        content.ref_on_resize({
            // Content height change
            let state = state.weak();
            let old_content_height = Cell::new(-1.0f64);
            move |_, _, content_height| {
                logd!("EV content resize");
                let Some(state) = state.upgrade() else {
                    return;
                };
                if content_height == old_content_height.get() {
                    return;
                }
                let mut self1 = state.0.borrow_mut();
                old_content_height.set(content_height);
                logd!("reset scroll to {} - ? / {}", self1.logical_scroll_top, self1.logical_content_height);
                self1.frame.raw().set_scroll_top(self1.logical_scroll_top.round() as i32);
                self1.mute_scroll = Utc::now() + Duration::milliseconds(50);
            }
        });
        state.shake_immediate();
        return state;
    }

    pub fn weak(&self) -> WeakInfiniscroll<Id> {
        return WeakInfiniscroll(Rc::downgrade(&self.0));
    }

    pub fn el(&self) -> El {
        return self.0.borrow().frame.clone();
    }

    pub fn jump(&self, time: Id) {
        {
            let mut self1 = self.0.borrow_mut();
            self1.reset_time = time;
            self1.real.clear();
            self1.anchor_i = None;
            self1.anchor_alignment = 0.5;
            self1.anchor_offset = 0.;
            self1.early_sticky.clear();
            self1.late_sticky.clear();
            for f in self1.feeds.values_mut() {
                f.early_reserve.clear();
                f.late_reserve.clear();
                f.early_stop = false;
                f.late_stop = false;
                f.initial = true;
            }
        }
        self.shake_immediate();
    }

    pub fn sticky(&self, feed_id: FeedId, id: Id) {
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            self1.sticky_set.insert(id.clone());
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            for e in &self1.early_sticky {
                if e.entry.time() == id {
                    return;
                }
            }
            for e in &self1.late_sticky {
                if e.entry.time() == id {
                    return;
                }
            }
            for e in feed.early_reserve.iter().rev() {
                let e_time = e.time();
                if e_time == id {
                    let e_state = realize_entry(&self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                    let mut insert_before = 0;
                    for (i, e) in self1.early_sticky.iter().enumerate() {
                        if id < e.entry.time() {
                            insert_before = i;
                            break;
                        }
                    }
                    self1.early_sticky.splice(insert_before, 0, vec![e_state]);
                    return;
                }
            }
            for e in feed.late_reserve.iter().rev() {
                let e_time = e.time();
                if e_time == id {
                    let e_state = realize_entry(&self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                    let mut insert_before = self1.late_sticky.len();
                    for (i, e) in self1.late_sticky.iter().enumerate() {
                        if id < e.entry.time() {
                            insert_before = i;
                            break;
                        }
                    }
                    self1.late_sticky.splice(insert_before, 0, vec![e_state]);
                    return;
                }
            }
        }
        self.shake();
    }

    pub fn unsticky(&self, id: Id) {
        let mut found = false;
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            self1.sticky_set.remove(&id);
            {
                let mut found_early = None;
                for (i, e) in self1.early_sticky.iter().enumerate() {
                    if e.entry.time() != id {
                        continue;
                    }
                    found_early = Some(i);
                    break;
                }
                if let Some(i) = found_early {
                    self1.early_sticky.remove(i);
                    found = true;
                }
            }
            {
                let mut found_late = None;
                for (i, e) in self1.late_sticky.iter().enumerate() {
                    if e.entry.time() != id {
                        continue;
                    }
                    found_late = Some(i);
                }
                if let Some(i) = found_late {
                    self1.late_sticky.remove(i);
                    found = true;
                }
            }
        }
        if found {
            self.shake();
        }
    }

    fn shake_immediate(&self) {
        logd!("shake immediate ------------");
        let mut self1 = self.0.borrow_mut();
        let self1 = &mut *self1;
        self1.delay_update_last = Utc::now();
        self1.delay_update = None;

        // # Calculate content + current theoretical used space
        let mut used_early = 0f64;
        let mut used_late = 0f64;
        let mut origin_y = 0f64;
        if !self1.real.is_empty() {
            let real_height = self1.real.el().offset_height();
            let anchor_i = self1.anchor_i.unwrap();
            let anchor = &mut self1.real.get(anchor_i).unwrap();
            let anchor_top = anchor.entry_el.offset_top();
            let anchor_height = anchor.entry_el.offset_height();
            origin_y = anchor_top + anchor_height * self1.anchor_alignment
                // Shift up becomes early usage
                - self1.anchor_offset;
            used_early = origin_y;
            used_late = real_height - origin_y;
        }
        logd!("shake imm, used early {}, late {}", used_early, used_late);

        // # Realize and unrealize elements to match goal bounds
        //
        // ## Early...
        let want_nostop_early = BUFFER + self1.cached_frame_height * self1.anchor_alignment;
        let mut unrealize_early = 0usize;
        for e in &self1.real {
            let bottom = e.entry_el.offset_top() + e.entry_el.offset_height();
            let min_dist = origin_y - bottom;
            if min_dist <= want_nostop_early {
                break;
            }
            logd!("unrealize; bottom {} vs want early {}", min_dist, want_nostop_early);
            unrealize_early += 1;
            used_early = origin_y - bottom;
        }
        let mut stop_all_early = true;
        let mut realized_early = vec![];

        bb!{
            'realize_early _;
            while used_early < want_nostop_early {
                let mut use_feed = None;
                for (feed_id, f_state) in &self1.feeds {
                    let Some(entry) = f_state.early_reserve.front() else {
                        // Reserve empty
                        if f_state.early_stop {
                            continue;
                        } else {
                            // Pending more
                            stop_all_early = false;
                            break 'realize_early;
                        }
                    };
                    let replace = match &use_feed {
                        Some((_, time)) => {
                            entry.time() > *time
                        },
                        None => {
                            true
                        },
                    };
                    if replace {
                        use_feed = Some((feed_id.clone(), entry.time()));
                    }
                }
                let Some((feed_id, _)) = use_feed else {
                    break 'realize_early;
                };
                let feed = self1.feeds.get_mut(&feed_id).unwrap();
                let entry = feed.early_reserve.pop_front().unwrap();
                let mut real = None;
                if let Some(f) = self1.early_sticky.last() {
                    if f.entry.time() == entry.time() {
                        let real1 = self1.early_sticky.pop().unwrap();
                        real1.entry_el.ref_remove();
                        real = Some(real1);
                    }
                }
                let real =
                    real.unwrap_or_else(
                        || realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry),
                    );
                self1.real.el().ref_push(real.entry_el.clone());
                let height = real.entry_el.offset_height();
                real.entry_el.ref_remove();
                used_early += height;
                logd!("realize pre; id {:?}; height {} -> {}", real.entry.time(), height, used_early);
                realized_early.push(real);
            }
            stop_all_early = false;
        };

        // ## Late...
        let want_nostop_late = BUFFER + self1.cached_frame_height * (1. - self1.anchor_alignment);
        let mut unrealize_late = 0usize;
        for e in self1.real.iter().rev() {
            let top = e.entry_el.offset_top();
            let min_dist = top - origin_y;
            if min_dist <= want_nostop_late {
                break;
            }
            unrealize_late += 1;
            used_late = top - origin_y;
        }
        let mut stop_all_late = true;
        let mut realized_late = vec![];

        bb!{
            'realize_late _;
            while used_late < want_nostop_late {
                let mut use_feed = None;
                for (feed_id, f_state) in &self1.feeds {
                    let Some(entry) = f_state.late_reserve.front() else {
                        // Reserve empty
                        if f_state.late_stop {
                            continue;
                        } else {
                            // Pending more
                            stop_all_late = false;
                            break 'realize_late;
                        }
                    };
                    let replace = match &use_feed {
                        Some((_, time)) => {
                            entry.time() < *time
                        },
                        None => {
                            true
                        },
                    };
                    if replace {
                        use_feed = Some((feed_id.clone(), entry.time()));
                    }
                }
                let Some((feed_id, _)) = use_feed else {
                    break 'realize_late;
                };
                let feed = self1.feeds.get_mut(&feed_id).unwrap();
                let entry = feed.late_reserve.pop_front().unwrap();
                let mut real = None;
                if let Some(f) = self1.late_sticky.first() {
                    if f.entry.time() == entry.time() {
                        let real1 = self1.late_sticky.remove(0);
                        real1.entry_el.ref_remove();
                        real = Some(real1);
                    }
                }
                let real =
                    real.unwrap_or_else(
                        || realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry),
                    );
                self1.content.ref_push(real.entry_el.clone());
                let height = real.entry_el.offset_height();
                real.entry_el.ref_remove();
                used_late += height;
                logd!("realize post; id {:?}; height {} -> {}", real.entry.time(), height, used_late);
                realized_late.push(real);
            }
            stop_all_late = false;
        };

        // ## Apply changes
        //
        // ### Update anchor
        match self1.anchor_i {
            Some(anchor_i) => {
                logd!("prepend; shift anchor {} +{} -{}", anchor_i, realized_early.len(), unrealize_early);
                self1.anchor_i = Some(anchor_i + realized_early.len() - unrealize_early);
            },
            None => {
                match (realized_early.is_empty(), realized_late.is_empty()) {
                    (true, true) => {
                        // nop
                    },
                    (true, false) => {
                        logd!("prepend; reset anchor i -> {:?} (first)", realized_early.len());
                        self1.anchor_i = Some(0);
                    },
                    (false, _) => {
                        logd!("prepend; reset anchor i -> {:?} (last)", realized_early.len() - 1);
                        self1.anchor_i = Some(realized_early.len() - 1);
                    },
                }
            },
        }

        // ### Early elements
        //
        // late to early -> early to late
        realized_early.reverse();
        for e_state in self1.real.splice(0, unrealize_early, realized_early) {
            let feed = self1.feeds.get_mut(&e_state.feed_id).unwrap();
            feed.early_reserve.push_front(e_state.entry.clone());
            if self1.sticky_set.contains(&e_state.entry.time()) {
                self1.early_sticky.push(e_state);
            }
        }

        // ### Late elements
        let mut late_prepend_sticky = vec![];
        for e_state in self1.real.splice(self1.real.len() - unrealize_late, unrealize_late, realized_late).rev() {
            let feed = self1.feeds.get_mut(&e_state.feed_id).unwrap();
            feed.late_reserve.push_front(e_state.entry.clone());
            if self1.sticky_set.contains(&e_state.entry.time()) {
                late_prepend_sticky.push(e_state);
            }
        }
        self1.late_sticky.splice(0, 0, late_prepend_sticky);
        if let Some(anchor_i) = &self1.anchor_i {
            logd!("anchor origin now {}", {
                let base_e_content_offset = self1.logical_content_layout_offset + self1.cached_real_offset;
                let anchor = self1.real.get(*anchor_i).unwrap();
                base_e_content_offset + anchor.entry_el.offset_top() +
                    anchor.entry_el.offset_height() * self1.anchor_alignment -
                    self1.anchor_offset
            });
        }

        // # Prune reserve and unset stop status
        let mut requesting_early = false;
        let mut requesting_late = false;
        for (feed_id, f_state) in &mut self1.feeds {
            if f_state.initial {
                f_state.feed.request_around(self1.reset_time.clone(), REQUEST_COUNT);
                requesting_early = true;
                requesting_late = true;
            } else {
                if f_state.early_reserve.len() > MAX_RESERVE {
                    f_state.early_reserve.truncate(MAX_RESERVE);
                    f_state.early_stop = false;
                }
                if !f_state.early_stop && f_state.early_reserve.len() < MIN_RESERVE {
                    let pivot = get_pivot_early(&self1.real, *feed_id, f_state).unwrap();
                    logd!("request early (pivot {:?})", pivot);
                    f_state.feed.request_before(pivot, REQUEST_COUNT);
                    requesting_early = true;
                }
                if f_state.late_reserve.len() > MAX_RESERVE {
                    f_state.late_reserve.truncate(MAX_RESERVE);
                    f_state.late_stop = false;
                }
                if !f_state.late_stop && f_state.late_reserve.len() < MIN_RESERVE {
                    let pivot = get_pivot_late(&self1.real, *feed_id, f_state).unwrap();
                    logd!("request late (pivot {:?})", pivot);
                    f_state.feed.request_after(pivot, REQUEST_COUNT);
                    requesting_late = true;
                }
            }
        }
        self1.center_spinner.ref_modify_classes(&[(CSS_HIDE, self1.real.is_empty() && requesting_early)]);
        self1.early_spinner.ref_modify_classes(&[(CSS_HIDE, !self1.real.is_empty() && requesting_early)]);
        self1.late_spinner.ref_modify_classes(&[(CSS_HIDE, !self1.real.is_empty() && requesting_late)]);
        self1.cached_real_offset = self1.real.el().offset_top();

        // # Update alignment based on used space, stop states
        self1.transition_alignment_reanchor();

        // # Calculate desired space per used space + stop status
        //
        // Distance from content-origin to start of content
        let want_early;
        if stop_all_early {
            want_early = used_early.min(want_nostop_early);
        } else {
            want_early = want_nostop_early;
        }

        // Distance from content-origin to end of content
        let want_late;
        if stop_all_late {
            want_late = used_late.min(want_nostop_late);
        } else {
            want_late = want_nostop_late;
        }
        logd!(
            "stop all early {}, stop all late{}; want early {}, want late {}",
            stop_all_early,
            stop_all_late,
            want_early,
            want_late
        );

        // # Update logical height, deferred real height update
        let new_height = (want_early + want_late).max(self1.cached_frame_height);
        if (new_height - self1.logical_content_height).abs() >= 1. {
            logd!("requesting height {} -> {}", self1.logical_content_height, new_height);
            self1.logical_content_height = new_height;
            self1
                .content
                .raw()
                .dyn_ref::<HtmlElement>()
                .unwrap()
                .style()
                .set_property("height", &format!("{}px", new_height))
                .unwrap();
        }

        // # Position content based on content size, used space, and alignment
        self1.logical_content_layout_offset = self1.anchor_alignment.mix(
            // Start of content is origin (want_early) minus the amount used before
            // (used_early)
            want_early - used_early - self1.cached_real_offset,
            // Backwards from end of content, in case used < frame height.
            // `logical_content_height` is padded, so this will push it to the end.
            self1.logical_content_height - want_late - used_early - self1.cached_real_offset,
        );
        logd!(
            "content layout top: mix by {} ({} - {} - {} = {}, {} - {} - {} - {} = {}) = {}",
            self1.anchor_alignment,
            want_early,
            used_early,
            self1.cached_real_offset,
            want_early - used_early - self1.cached_real_offset,
            self1.logical_content_height,
            want_late,
            used_early,
            self1.cached_real_offset,
            self1.logical_content_height - want_late - used_early - self1.cached_real_offset,
            self1.logical_content_layout_offset
        );
        self1
            .content_layout
            .raw()
            .dyn_ref::<HtmlElement>()
            .unwrap()
            .style()
            .set_property("top", &format!("{}px", self1.logical_content_layout_offset))
            .unwrap();

        // # Calculate centered logical scroll
        self1.update_logical_scroll(want_early, want_late);

        // # Set scroll (may fail, gets fixed after resize)
        self1.frame.raw().set_scroll_top(self1.logical_scroll_top.round() as i32);
        self1.mute_scroll = Utc::now() + Duration::milliseconds(50);
        logd!("==============");
    }

    fn shake(&self) {
        let mute_scroll = self.0.borrow().mute_scroll >= Utc::now();
        if mute_scroll {
            self.0.borrow_mut().delay_update = Some(Timeout::new(0, {
                let state = self.weak();
                move || {
                    let Some(state) = state.upgrade() else {
                        return;
                    };
                    state.shake_immediate();
                }
            }));
        } else {
            self.0.borrow_mut().delay_update = Some(Timeout::new(200, {
                let state = self.weak();
                move || {
                    let Some(state) = state.upgrade() else {
                        return;
                    };
                    state.shake_immediate();
                }
            }));
        }
    }

    pub fn add_entries_around_initial(
        &self,
        feed_id: FeedId,
        pivot: Id,
        entries: Vec<Rc<dyn Entry<Id>>>,
        early_stop: bool,
        late_stop: bool,
    ) {
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            if pivot != self1.reset_time {
                return;
            }
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            if !feed.initial {
                return;
            }
            feed.initial = false;

            // early to late
            let mut prepend = vec![];

            // early to late
            let mut postpend = vec![];
            for e in entries {
                let time = e.time();
                if time < self1.reset_time {
                    prepend.push(e);
                } else if time == self1.reset_time {
                    let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, e);
                    logd!("realize initial anchor; id {:?}", real.entry.time());
                    self1.real.push(real);
                    self1.anchor_i = Some(0);
                } else {
                    postpend.push(e);
                }
            }
            for e in &prepend {
                if self1.sticky_set.contains(&e.time()) {
                    let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                    self1.early_sticky.push(real);
                }
            }
            prepend.reverse();
            feed.early_reserve.extend(prepend);
            for e in &postpend {
                if self1.sticky_set.contains(&e.time()) {
                    let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                    self1.late_sticky.push(real);
                }
            }
            feed.late_reserve.extend(postpend);
            feed.early_stop = early_stop;
            feed.late_stop = late_stop;
            self1.transition_alignment_reanchor();
        }
        self.shake();
    }

    /// Entries must be sorted latest to earliest.
    pub fn add_entries_before_nostop(
        &self,
        feed_id: FeedId,
        initial_pivot: Id,
        entries: Vec<Rc<dyn Entry<Id>>>,
        stop: bool,
    ) {
        assert!(bb!{
            'assert _;
            let mut at = initial_pivot.clone();
            for e in &entries {
                if e.time() >= at {
                    break 'assert false;
                }
                at = e.time();
            }
            true
        });
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            let Some(current_pivot) = get_pivot_early(&self1.real, feed_id, self1.feeds.get(&feed_id).unwrap()) else {
                return;
            };
            if initial_pivot != current_pivot {
                return;
            }
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            logd!(
                "??? add entries before nostop, pivot {:?}: {:?} -> {:?}",
                initial_pivot,
                entries.first().unwrap().time(),
                entries.last().unwrap().time()
            );
            let mut prepend_sticky = vec![];
            for e in entries.iter().rev() {
                if self1.sticky_set.contains(&e.time()) {
                    let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                    prepend_sticky.push(real);
                }
            }
            self1.early_sticky.splice(0, 0, prepend_sticky);
            feed.early_reserve.extend(entries);
            feed.early_stop = stop;
            self1.transition_alignment_reanchor();
        }
        self.shake();
    }

    /// Entries must be sorted earliest to latest.
    pub fn add_entries_after_nostop(
        &self,
        feed_id: FeedId,
        initial_pivot: Id,
        entries: Vec<Rc<dyn Entry<Id>>>,
        stop: bool,
    ) {
        assert!(bb!{
            'assert _;
            let mut at = initial_pivot.clone();
            for e in &entries {
                if e.time() <= at {
                    break 'assert false;
                }
                at = e.time();
            }
            true
        });
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            let Some(current_pivot) = get_pivot_late(&self1.real, feed_id, self1.feeds.get(&feed_id).unwrap()) else {
                return;
            };
            if initial_pivot != current_pivot {
                return;
            }
            let feed = self1.feeds.get_mut(&feed_id).unwrap();
            logd!(
                "??? add entries after nostop, pivot {:?}: {:?} -> {:?}",
                initial_pivot,
                entries.first().unwrap().time(),
                entries.last().unwrap().time()
            );
            for e in entries.iter() {
                if self1.sticky_set.contains(&e.time()) {
                    let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, e.clone());
                    self1.late_sticky.push(real);
                }
            }
            feed.late_reserve.extend(entries);
            feed.late_stop = stop;
            self1.transition_alignment_reanchor();
        }
        self.shake();
    }

    pub fn add_entry_after_stop(&self, feed_id: FeedId, entry: Rc<dyn Entry<Id>>) {
        {
            let mut self1 = self.0.borrow_mut();
            let self1 = &mut *self1;
            let mut stop_all = true;
            let mut reserves_empty_all = true;
            for f in self1.feeds.values() {
                stop_all = stop_all && f.late_stop;
                reserves_empty_all = reserves_empty_all && f.late_reserve.is_empty();
            }
            if stop_all {
                let feed = self1.feeds.get_mut(&feed_id).unwrap();
                if !reserves_empty_all {
                    // Some feeds have unrealized elements, can't realize yet
                    logd!("realtime; stopped, adding to reserve");
                    if feed.late_reserve.len() < MAX_RESERVE {
                        feed.late_reserve.push_back(entry);
                        logd!("realtime, push late reserve");
                    } else {
                        logd!("realtime, stop but full, discard, now not stop");
                        feed.late_stop = false;
                    }
                } else {
                    // All feeds stopped, all elements realized, this is the next element so realize
                    // immediately
                    let time = entry.time();
                    let insert_before_i = bb!{
                        'find_insert _;
                        for (i, real_state) in self1.real.iter().enumerate().rev() {
                            if time > real_state.entry.time() {
                                break 'find_insert i + 1;
                            }
                        }
                        break 0;
                    };
                    if insert_before_i == self1.real.len() {
                        // Insert at end
                        let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
                        let anchor_i = self1.anchor_i.unwrap();
                        if anchor_i == self1.real.len() - 1 {
                            self1.anchor_i = Some(anchor_i + 1);
                            logd!("realtime, anchor_i reset to last {:?}", self1.anchor_i);
                        }
                        self1.real.push(real);
                    } else if insert_before_i == 0 {
                        // Insert at start of early reserve, because insertion is unbounded within
                        // realized elements (shake will realize it if necessary) OR no real elements
                        logd!("realtime, push early reserve");
                        feed.early_reserve.push_front(entry);
                        if feed.early_reserve.len() > MAX_RESERVE {
                            feed.early_reserve.truncate(MAX_RESERVE);
                            feed.early_stop = false;
                        }
                    } else {
                        // Insert within real elements
                        let real = realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry);
                        let anchor_i = self1.anchor_i.unwrap();
                        if insert_before_i <= anchor_i {
                            self1.anchor_i = Some(anchor_i + 1);
                            logd!("realtime, insert before; anchor_i {:?}", self1.anchor_i);
                        }
                        self1.real.insert(insert_before_i, real);
                    }
                }
            } else {
                // Some feeds not stopped
                let feed = self1.feeds.get_mut(&feed_id).unwrap();
                if !feed.late_stop {
                    // This feed not stopped; might be a gap, discard - will be fetched in turn
                    logd!("realtime, not stopped, discard");
                    return;
                }

                // This feed is stop, so add to reserve
                if feed.late_reserve.len() < MAX_RESERVE {
                    if self1.sticky_set.contains(&entry.time()) {
                        let real =
                            realize_entry(self1.entry_resize_observer.as_ref().unwrap(), feed_id, entry.clone());
                        self1.late_sticky.push(real);
                    }
                    feed.late_reserve.push_back(entry);
                    logd!("realtime, push late reserve");
                } else {
                    logd!("realtime, stop but full, discard, now not stop");
                    feed.late_stop = false;
                }
            }
        }
        self.shake();
    }
}
