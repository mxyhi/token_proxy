//! my_stats_api 单测：四形态 token 语义 / 端口推导 / 鉴权判定 / 回写不污染。
use super::*;
use crate::ProxyConfigFile;

/// 构造带 `my_stats_api` 段的完整配置（host/port 为必填锚点字段）。
fn config_with(section_json: &str, proxy_port: u16) -> ProxyConfigFile {
    let json = format!(
        r#"{{ "host": "127.0.0.1", "port": {proxy_port}, "my_stats_api": {section_json} }}"#
    );
    serde_json::from_str(&json).expect("config.jsonc 片段应可解析")
}

#[test]
fn 段缺失解析为none且回写不含该键() {
    let c: ProxyConfigFile =
        serde_json::from_str(r#"{"host":"127.0.0.1","port":9208}"#).unwrap();
    assert!(c.my_stats_api.is_none());
    // 回写不污染：缺省（None）不出现在序列化输出中
    let json = serde_json::to_string(&ProxyConfigFile::default()).unwrap();
    assert!(!json.contains("my_stats_api"), "skip_serializing_if 应生效");
}

#[test]
fn token_四形态语义() {
    // 缺失 / "" / null → 无鉴权：任何 provided（含错误值）都通过
    for section in [
        r#"{}"#,
        r#"{"token": null}"#,
        r#"{"token": ""}"#,
        r#"{"token": "   "}"#,
    ] {
        let c = config_with(section, 9208);
        let r = c.my_stats_api.as_ref().unwrap();
        let resolved = resolve_section(r, 9208);
        assert!(resolved.token.is_none(), "section={section}");
        assert!(auth_ok(&resolved, Some("anything")), "section={section}");
        assert!(auth_ok(&resolved, None), "section={section}");
    }
    // "abc" → 必须匹配
    let c = config_with(r#"{"token": "abc"}"#, 9208);
    let resolved = resolve_section(c.my_stats_api.as_ref().unwrap(), 9208);
    assert_eq!(resolved.token.as_deref(), Some("abc"));
    assert!(auth_ok(&resolved, Some("abc")));
    assert!(auth_ok(&resolved, Some(" abc ")), "提供值容忍首尾空白");
    assert!(!auth_ok(&resolved, Some("bad")));
    assert!(!auth_ok(&resolved, None));
}

#[test]
fn 端口缺省推导为代理端口加一百_显式配置覆盖() {
    // 缺省：9208 + 100 = 9308
    let c = config_with(r#"{"enabled": true}"#, 9208);
    let r = resolve_section(c.my_stats_api.as_ref().unwrap(), 9208);
    assert_eq!(r.port, 9308);
    assert!(r.enabled);
    // 显式覆盖
    let c = config_with(r#"{"port": 1234}"#, 9208);
    let r = resolve_section(c.my_stats_api.as_ref().unwrap(), 9208);
    assert_eq!(r.port, 1234);
    // 代理端口 + 100 溢出 u16 → 饱和封顶 65535
    let c = config_with(r#"{"enabled": true}"#, 65500);
    let r = resolve_section(c.my_stats_api.as_ref().unwrap(), 65500);
    assert_eq!(r.port, 65535);
}

#[test]
fn host_缺省回环_显式透传() {
    let c = config_with(r#"{"enabled": true}"#, 9208);
    let r = resolve_section(c.my_stats_api.as_ref().unwrap(), 9208);
    assert_eq!(r.host, "127.0.0.1");
    let c = config_with(r#"{"host": "0.0.0.0"}"#, 9208);
    let r = resolve_section(c.my_stats_api.as_ref().unwrap(), 9208);
    assert_eq!(r.host, "0.0.0.0");
    // 空白 host 视同缺省
    let c = config_with(r#"{"host": "  "}"#, 9208);
    let r = resolve_section(c.my_stats_api.as_ref().unwrap(), 9208);
    assert_eq!(r.host, "127.0.0.1");
}

#[test]
fn 同步全局_读取一致() {
    let mut c = ProxyConfigFile::default();
    c.my_stats_api = Some(MyStatsApiSection {
        enabled: Some(true),
        host: Some("127.0.0.1".into()),
        port: Some(19308),
        token: Some("t1".into()),
    });
    let synced = sync_settings(&c);
    assert_eq!(synced.as_ref().and_then(|r| r.token.as_deref()), Some("t1"));
    let cur = current_settings().expect("同步后应可读");
    assert_eq!(cur.port, 19308);
    // 二次 sync 覆盖旧值（RwLock 内层语义，区别于裸 OnceLock）
    c.my_stats_api.as_mut().unwrap().token = Some("t2".into());
    sync_settings(&c);
    assert_eq!(
        current_settings().and_then(|r| r.token),
        Some("t2".into()),
        "reload 必须真正覆盖旧值"
    );
    // 清理：恢复为 None，避免污染其它测试的全局态
    c.my_stats_api = None;
    sync_settings(&c);
    assert!(current_settings().is_none());
}

#[test]
fn 恒时比较_长度不等立即拒绝() {
    let r = MyStatsApiResolved {
        enabled: true,
        host: "127.0.0.1".into(),
        port: 9308,
        token: Some("abc".into()),
    };
    assert!(!auth_ok(&r, Some("ab")));
    assert!(!auth_ok(&r, Some("abcd")));
}
