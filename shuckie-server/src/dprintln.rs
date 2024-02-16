macro_rules! dprintln {
    ($($what: tt)*) => {
        #[cfg(debug_assertions)]
        println!($($what)*);
    }
}
