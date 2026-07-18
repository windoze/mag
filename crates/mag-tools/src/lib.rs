#![warn(missing_docs)]

//! Tool registry and built-in tool integration points for mag.

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_tools_crate_is_testable() {
        assert_eq!("mag-tools", env!("CARGO_PKG_NAME"));
    }
}
