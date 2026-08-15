# デバッグ記録: Tokioのタイムアウト後にワーカーが状態を更新する

## 目的

Rust 1.75.0とTokioで、`JoinHandle`を`timeout`へ値として渡すことにより、期限切れ後もワーカーが実行を続ける理由を、実行可能な最小例で確認します。

> 契約: ワーカーの待機が期限切れになった場合、`DeadlineExceeded`を返し、ワーカーは後から状態を更新しない。バグ状態では、`DeadlineExceeded`を返した後にワーカーが`Completed`へ更新する。

## 実行環境と再現境界

| 項目 | 内容 |
| --- | --- |
| 言語処理系 | Rust 1.75.0 |
| 難易度プロファイル | 実践・上級。呼び出し元の戻り値だけではワーカーの存続を判断できず、別の最終状態を観測する必要があるため |
| ビルド・テスト方法 | `cargo test -- --nocapture` |
| 使用する依存関係 | Tokio 1.36以上。テストランナーはRust標準の組み込みテスト |
| 使用しないもの | Webフレームワーク、DB、外部サービス、任意の`sleep`による順序制御 |
| 公開境界 | `run_job_with_deadline` |
| 最終観測 | 戻り値、制御メッセージの受理可否、共有状態 |
| 決定性の確保 | `Barrier`でワーカーが`oneshot`受信待ちへ入ったことを同期し、期限切れ後にだけ制御メッセージを送る |

この境界を選んだ理由は、HTTPや永続化を介さずに、Tokioの待機とタスク中断の契約を直接観測できるためです。

## 最初に観測した事実

| 観測順 | 事実 | 得られた証拠 |
| --- | --- | --- |
| 1 | ワーカーは`Barrier`通過後、制御メッセージを待機していた。 | `[worker] 起動を通知し、制御メッセージを待機します`というログ |
| 2 | 呼び出し元は期限切れで`DeadlineExceeded`を返した。 | `[deadline] 期限切れとして呼び出し元へ返します`というログと戻り値アサーション |
| 3 | 期限切れ後に制御メッセージが受理された。 | `release_tx.send(()).is_ok()`が真になったこと |
| 4 | ワーカーは`Completed`へ状態を更新した。 | `[worker] 状態を Completed に更新しました`というログと最終状態アサーション |

バグ状態のコミットは`b8b43c6`です。次のコマンドを実行すると、設定やコンパイルではなく、意図した状態差分で失敗します。

```bash
cargo test deadline_expiry_must_not_allow_a_late_state_change -- --nocapture
```

実際の失敗は次のとおりでした。

```text
[worker] 起動を通知し、制御メッセージを待機します
[deadline] 期限切れとして呼び出し元へ返します
[worker] 制御メッセージを受信しました
[worker] 状態を Completed に更新しました
期限切れ後もワーカーが制御メッセージを受け付け、状態を更新しました: Completed
```

GDBでバグ状態の期限切れ分岐にブレークポイントを置くと、`src/lib.rs:42`の期限切れログで停止しました。スタック先頭は`rust_tokio_timeout_detach_debug_lab::run_job_with_deadline::{async_fn#0}`であり、タイムアウトを返す経路が実際に実行されたことを確認しました。その後に同じテストを継続すると、ワーカーの状態更新によるアサーション失敗を観測できます。

```bash
test_bin=$(find target/debug/deps -maxdepth 1 -type f -executable -name 'rust_tokio_timeout_detach_debug_lab-*' | head -n 1)
gdb --batch \
  -ex 'break src/lib.rs:41' \
  -ex 'run --exact tests::deadline_expiry_must_not_allow_a_late_state_change --nocapture' \
  -ex 'bt 8' \
  --args "$test_bin"
```

## 競合仮説と検証

| 仮説 | 予測 | 検証 | 結果 |
| --- | --- | --- | --- |
| `timeout`が`JoinHandle`を落とし、タスクをデタッチしている | 期限切れ後もワーカーの受信側が生き、解放後に状態更新する | バグ状態のログ、送信成功、最終状態を確認する | 支持 |
| ワーカーが期限切れより前に状態更新している | 期限切れの前に`Completed`になる | `Barrier`と`oneshot`で、テスト側が明示的に解放するまで更新できないようにする | 除外 |
| `Mutex`の読み出しが遅れ、古い状態を誤読している | 完了通知後も`Pending`が読まれる | 完了通知を待ってから状態を読む | 除外 |

## 確定した原因

`timeout`は期限内に与えられたFutureが完了しなければエラーを返し、与えられたFutureをキャンセルします。[1] バグ実装は`JoinHandle`をそのFutureとして値渡ししていました。そのため期限切れ時には`JoinHandle`がドロップされます。

`JoinHandle`はドロップ時に関連タスクをデタッチし、タスクはバックグラウンドで継続します。[2] よって、呼び出し元の期限切れという直接結果と、ワーカーによる後続の状態更新という最終状態が同時に成立しました。これはラボで観測した事実であり、ドロップとデタッチの一般則はTokio公式ドキュメントで裏づけています。[1] [2]

## 最小修正

`JoinHandle`を`mut`で保持し、`timeout(deadline, &mut handle)`へ参照として渡します。期限切れ時にもハンドルを所有したままにできるため、`handle.abort()`を呼び、その完了を`handle.await`で確認します。

この修正は`JoinHandle`をデタッチする直接原因だけを対象にします。APIの変更、追加の依存関係、チャネル方式の変更、タスク監視基盤の導入は含めていません。修正コミットは`b05b89a`です。

## 回帰保証

| 守ること | テストまたは診断 | 修正後の結果 |
| --- | --- | --- |
| 期限切れが`DeadlineExceeded`を返す | `deadline_expiry_must_not_allow_a_late_state_change` | 成功 |
| 期限切れ後にワーカーが状態を変えない | 同テストで送信が失敗し、最終状態が`Pending`であることを確認 | 成功 |
| 期限内に解放されたワーカーは正常完了する | `worker_completes_when_released_before_deadline` | 成功 |
| コード形式が標準に従う | `cargo fmt --check` | 成功 |

固定済みの状態で、`cargo test -- --nocapture`を実行し、ユニットテスト2件とドキュメントテストがすべて成功することを確認しました。

## 再現手順

```bash
# 修正済み状態を検証する
cargo fmt --check
cargo test -- --nocapture

# バグ状態を確認する。作業中の変更は先に退避する
git switch --detach b8b43c6
cargo test deadline_expiry_must_not_allow_a_late_state_change -- --nocapture

# 修正済み状態へ戻る
git switch main
```

## スコープと注意点

このラボは、`tokio::spawn`で作成した非ブロッキングタスクを`JoinHandle`で待機する条件に限って再現・修正を確認しています。`spawn_blocking`で起動した処理は同じ方法では中断できません。[2] また、タスクの中断はすでに永続化された副作用をロールバックしません。実際のアプリケーションでは、外部副作用に対する補償、冪等性、明示的なキャンセル伝播を別途設計する必要があります。

## References

[1] [Tokio `timeout` documentation](https://docs.rs/tokio/latest/tokio/time/fn.timeout.html)

[2] [Tokio `JoinHandle` documentation](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html)
