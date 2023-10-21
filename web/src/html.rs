use rooting::{
    el,
    El,
};

pub fn hbox(children: Vec<El>) -> El {
    return el("div").classes(&["hbox"]).extend(children);
}

pub fn vbox(children: Vec<El>) -> El {
    return el("div").classes(&["vbox"]).extend(children);
}

pub fn stack(children: Vec<El>) -> El {
    return el("div").classes(&["stack"]).extend(children);
}
