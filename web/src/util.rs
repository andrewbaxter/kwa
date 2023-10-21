#[macro_export]
macro_rules! bb{
    ($l: lifetime _; $($t: tt) *) => {
        $l: loop {
            #[allow(unreachable_code)] break {
                $($t) *
            };
        }
    };
    ($($t: tt) *) => {
        loop {
            #[allow(unreachable_code)] break {
                $($t) *
            };
        }
    };
}

#[macro_export]
macro_rules! log{
    ($t: literal $(, $a: expr) *) => {
        web_sys::console::log_1(&format!($t $(, $a) *).into());
    };
}

#[macro_export]
macro_rules! logd{
    ($t: literal $(, $a: expr) *) => {
        web_sys::console::log_1(&format!($t $(, $a) *).into());
    };
}

#[macro_export]
macro_rules! logn{
    ($t: literal $(, $a: expr) *) => {
    };
}
