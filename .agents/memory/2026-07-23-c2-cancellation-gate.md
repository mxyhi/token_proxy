# C2 cancellation / provider gate

- MutationWorker: unbounded channel + single tokio task; caller awaits oneshot; cancel/abort wait does not abort job
- claim_prepared_login: clone pending, clear only on complete/fail
- provider_mutation Mutex on kiro/codex/xai for all persist writes + snapshot/restore
- Kiro identity: provider+email (or profile_arn) overwrite on save_new_account/commit_login_record
- Tests: login_poll_cancel_still_commits_or_recovers, save_proxy_config_cancel_still_completes_cascade, provider_gate_serializes_save_and_lifecycle_restore, kiro_re_login_overwrites_same_identity
- Lock order xAI: provider → index → account → cache_sync; refresh_cache does NOT take provider (avoid re-entry with load_account)
