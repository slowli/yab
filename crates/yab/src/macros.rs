/// FIXME
#[macro_export]
macro_rules! captures {
    (|[$($arg:tt),+]| $block:block) => {{
        (
            [$(::core::stringify!($arg),)+],
            |[$($arg,)+]: [$crate::Capture; _]| $block,
        )
    }};
}
