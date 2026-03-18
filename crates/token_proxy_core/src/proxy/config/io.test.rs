use super::*;
use std::path::Path;

#[test]
fn parse_config_file_migrates_legacy_upstream_strategy_string() {
    let parsed = parse_config_file(
        r#"
        {
          "host": "127.0.0.1",
          "port": 9208,
          "upstream_strategy": "priority_fill_first",
          "upstreams": []
        }
        "#,
        Path::new("/tmp/config.jsonc"),
    )
    .expect("legacy config should migrate");

    assert!(parsed.migrated);
    assert_eq!(
        parsed.config.upstream_strategy.order,
        crate::proxy::config::UpstreamOrderStrategy::FillFirst
    );
    assert_eq!(
        parsed.config.upstream_strategy.dispatch,
        crate::proxy::config::UpstreamDispatchStrategy::Serial
    );
}
