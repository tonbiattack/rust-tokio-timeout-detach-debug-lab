# Tokioのタイムアウト後にワーカーが状態を更新する

RustとTokioで、期限切れを返した後もバックグラウンドワーカーが実行を続け、共有状態を更新してしまう不具合を再現します。失敗する契約テスト、実行ログ、デバッガー、最小修正、回帰テストを順に確認するための小さなデバッグラボです。

## この題材で守る契約

> ワーカーの完了を待つ処理が期限切れを返した場合、ワーカーは後から制御メッセージを受け付けず、共有状態を更新してはなりません。

バグ状態では、呼び出し元が`DeadlineExceeded`を返した後もワーカーがデタッチされたまま残り、`Pending`だった状態を`Completed`へ更新します。原因は、`timeout`の期限切れ時にドロップされる`JoinHandle`と、タスクを中断せずにデタッチする`JoinHandle`の挙動の組み合わせです。[1] [2]

## 最短の開始手順

修正済みの既定ブランチで、次を実行します。

```bash
cargo test -- --nocapture
```

期限切れのテストと、期限内の正常完了テストが成功します。期限切れのケースでは、ワーカーを中断したことを示す`JoinError::Cancelled`のログが出力されます。

## バグを再現する

バグ状態はコミット`b8b43c6`に保存しています。作業中の変更を退避してから、次を実行します。

```bash
git switch --detach b8b43c6
cargo test deadline_expiry_must_not_allow_a_late_state_change -- --nocapture
```

テストは、期限切れのログの後にワーカーが制御メッセージを受信し、`Completed`へ更新したことを示して失敗します。確認後は修正済みブランチへ戻ります。

```bash
git switch main
```

## 観測の要約

| 観測点 | バグ状態 | 修正後 |
| --- | --- | --- |
| 直接結果 | `DeadlineExceeded`を返す | `DeadlineExceeded`を返す |
| 期限切れ後の制御メッセージ | ワーカーが受理する | 受理できない |
| 最終状態 | `Completed`になる | `Pending`を保つ |
| 検証コマンド | 契約テストが失敗する | 同じ契約テストが成功する |

詳細な仮説、証拠、原因、修正、回帰保証は[`docs/debugging-record.md`](docs/debugging-record.md)に記録しています。題材の選定と再現設計は[`docs/topic-brief.md`](docs/topic-brief.md)を参照してください。

## 構成

```text
src/lib.rs                  実装と契約テスト
README.md                   開始手順
docs/topic-brief.md         題材と再現設計
docs/debugging-record.md    調査記録
```

## 前提条件

| 項目 | バージョンまたは条件 |
| --- | --- |
| Rust | 1.75以上 |
| Cargo | Rust同梱の標準ビルド・テストツール |
| Tokio | `Cargo.toml`で指定された1.36以上の互換版 |
| 外部サービス | 不要 |

## スコープ

このラボは、`tokio::spawn`で作成した非ブロッキングタスクを`JoinHandle`で待機し、その待機に`timeout`を適用する場合だけを扱います。`spawn_blocking`の停止、外部I/Oのキャンセル安全性、永続化済み副作用の補償、業務上のタイムアウト値は扱いません。

## References

[1] [Tokio `timeout` documentation](https://docs.rs/tokio/latest/tokio/time/fn.timeout.html)

[2] [Tokio `JoinHandle` documentation](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html)
