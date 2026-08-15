use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Barrier, Mutex};
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunError {
    DeadlineExceeded,
    WorkerPanicked,
}

pub async fn run_job_with_deadline(
    state: Arc<Mutex<JobState>>,
    worker_started: Arc<Barrier>,
    release_worker: oneshot::Receiver<()>,
    worker_completed: oneshot::Sender<()>,
    deadline: Duration,
) -> Result<(), RunError> {
    let handle = tokio::spawn(async move {
        worker_started.wait().await;
        eprintln!("[worker] 起動を通知し、制御メッセージを待機します");
        let _ = release_worker.await;
        eprintln!("[worker] 制御メッセージを受信しました");

        let mut state = state.lock().await;
        *state = JobState::Completed;
        eprintln!("[worker] 状態を Completed に更新しました");
        let _ = worker_completed.send(());
    });

    match timeout(deadline, handle).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(RunError::WorkerPanicked),
        Err(_) => {
            eprintln!("[deadline] 期限切れとして呼び出し元へ返します");
            Err(RunError::DeadlineExceeded)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deadline_expiry_must_not_allow_a_late_state_change() {
        let state = Arc::new(Mutex::new(JobState::Pending));
        let worker_started = Arc::new(Barrier::new(2));
        let (release_tx, release_rx) = oneshot::channel();
        let (completed_tx, completed_rx) = oneshot::channel();

        let runner = tokio::spawn(run_job_with_deadline(
            Arc::clone(&state),
            Arc::clone(&worker_started),
            release_rx,
            completed_tx,
            Duration::from_millis(20),
        ));

        worker_started.wait().await;

        let result = runner.await.expect("期限管理タスクが停止しました");
        assert_eq!(result, Err(RunError::DeadlineExceeded));

        let release_was_accepted = release_tx.send(()).is_ok();
        if release_was_accepted {
            completed_rx
                .await
                .expect("残存したワーカーが完了通知を送れませんでした");
        }

        let final_state = *state.lock().await;
        assert!(
            !release_was_accepted && final_state == JobState::Pending,
            "期限切れ後もワーカーが制御メッセージを受け付け、状態を更新しました: {final_state:?}"
        );
    }

    #[tokio::test]
    async fn worker_completes_when_released_before_deadline() {
        let state = Arc::new(Mutex::new(JobState::Pending));
        let worker_started = Arc::new(Barrier::new(2));
        let (release_tx, release_rx) = oneshot::channel();
        let (completed_tx, _completed_rx) = oneshot::channel();

        let runner = tokio::spawn(run_job_with_deadline(
            Arc::clone(&state),
            Arc::clone(&worker_started),
            release_rx,
            completed_tx,
            Duration::from_secs(1),
        ));

        worker_started.wait().await;
        release_tx
            .send(())
            .expect("起動済みワーカーへの制御メッセージ送信に失敗しました");

        let result = runner.await.expect("期限管理タスクが停止しました");
        assert_eq!(result, Ok(()));
        assert_eq!(*state.lock().await, JobState::Completed);
    }
}
