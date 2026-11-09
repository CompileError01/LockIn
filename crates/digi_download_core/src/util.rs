/// Builds pattern lazily.
#[macro_export]
macro_rules! regex_builder {
    ( $builder:expr ) => {{
        use regex::Regex;
        use std::sync::LazyLock;

        static EXPR: LazyLock<Regex> =
            LazyLock::new(|| $builder.build().expect("Regex failed to compile"));
        &EXPR
    }};
}

/// Lazy regex.
/// Only compiles once and prevents an extra variable.
#[macro_export]
macro_rules! regex {
    ( $regex:expr ) => {{
        use $crate::regex_builder;

        regex_builder!({
            use regex::RegexBuilder;
            RegexBuilder::new($regex)
        })
    }};
}
