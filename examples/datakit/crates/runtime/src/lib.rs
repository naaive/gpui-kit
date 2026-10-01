//! The one place DataKit runs Tokio.
//!
//! GPUI drives its own executors, and every crate in this repository uses
//! them. The mature database drivers, though, are written for Tokio: they
//! open Tokio sockets and spawn Tokio tasks. Rather than let Tokio leak into
//! views, DataKit keeps one multi-threaded Tokio runtime here and hands back
//! plain futures and streams that any executor can poll:
//!
//! - [`IoRuntime::spawn`] runs a future on Tokio and returns a
//!   [`RemoteTask`], a future of its result. Dropping the task aborts the
//!   work, so a GPUI `Task` that awaits it keeps GPUI's cancellation rule:
//!   drop the owner, and the query stops.
//! - [`IoRuntime::forward`] drives a stream on Tokio and returns a
//!   [`RemoteStream`] of its items, with a bounded buffer so a reader that
//!   stops reading stops the producer too.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use anyhow::anyhow;
use futures::{
    SinkExt as _, Stream, StreamExt as _,
    channel::{mpsc, oneshot},
};
use tokio::task::AbortHandle;

/// A Tokio runtime for database IO.
pub struct IoRuntime {
    runtime: tokio::runtime::Runtime,
}

impl IoRuntime {
    /// Start the runtime's worker threads.
    pub fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .thread_name("datakit-io")
            .worker_threads(2)
            .enable_all()
            .build()?;
        Ok(Self { runtime })
    }

    /// Run `future` on the runtime. Its result arrives through the returned
    /// task; dropping the task aborts the future.
    pub fn spawn<F, T>(&self, future: F) -> RemoteTask<T>
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let handle = self.runtime.spawn(async move {
            let _ = sender.send(future.await);
        });
        RemoteTask {
            receiver,
            abort: Some(handle.abort_handle()),
        }
    }

    /// Drive `stream` on the runtime, keeping at most `buffer` items ahead of
    /// the reader. Dropping the returned stream stops the producer and drops
    /// `stream` on the runtime, where its own cleanup can still do IO.
    pub fn forward<S>(&self, stream: S, buffer: usize) -> RemoteStream<S::Item>
    where
        S: Stream + Send + 'static,
        S::Item: Send + 'static,
    {
        let (mut sender, receiver) = mpsc::channel(buffer);
        let handle = self.runtime.spawn(async move {
            let mut stream = std::pin::pin!(stream);
            while let Some(item) = stream.next().await {
                if sender.send(item).await.is_err() {
                    break;
                }
            }
        });
        RemoteStream {
            receiver,
            abort: handle.abort_handle(),
        }
    }

    /// Run `future` to completion on the calling thread. For startup and
    /// tests; never call it from a GPUI executor thread.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }
}

/// The result of work running on an [`IoRuntime`].
///
/// Resolves to the work's own result, or to an error if the work panicked.
/// Dropping it aborts the work.
#[must_use = "dropping a RemoteTask aborts its work"]
pub struct RemoteTask<T> {
    receiver: oneshot::Receiver<anyhow::Result<T>>,
    /// `None` once detached.
    abort: Option<AbortHandle>,
}

impl<T> RemoteTask<T> {
    /// Let the work run to completion even though nobody will read its
    /// result.
    pub fn detach(mut self) {
        self.abort = None;
    }
}

impl<T> Future for RemoteTask<T> {
    type Output = anyhow::Result<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx).map(|result| {
            result.unwrap_or_else(|_| Err(anyhow!("the database task stopped unexpectedly")))
        })
    }
}

impl<T> Drop for RemoteTask<T> {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
    }
}

/// The items of a stream running on an [`IoRuntime`].
pub struct RemoteStream<T> {
    receiver: mpsc::Receiver<T>,
    abort: AbortHandle,
}

impl<T> Stream for RemoteStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}

impl<T> Drop for RemoteStream<T> {
    fn drop(&mut self) {
        // Closing the channel lets the forwarding task notice on its next
        // send and drop the stream itself, so a driver's cleanup (cancelling
        // the statement) runs inside the runtime. Aborting is the fallback
        // for a producer that is stuck waiting on the network.
        self.receiver.close();
        self.abort.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[test]
    fn a_spawned_future_runs_on_tokio_and_reports_its_result() {
        let runtime = IoRuntime::new().unwrap();
        let task = runtime.spawn(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(21 * 2)
        });
        assert_eq!(futures::executor::block_on(task).unwrap(), 42);
    }

    #[test]
    fn dropping_a_task_aborts_its_work() {
        let runtime = IoRuntime::new().unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        let task = runtime.spawn({
            let finished = finished.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                finished.store(true, Ordering::SeqCst);
                Ok(())
            }
        });
        drop(task);
        std::thread::sleep(Duration::from_millis(400));
        assert!(!finished.load(Ordering::SeqCst));
    }

    #[test]
    fn a_detached_task_finishes() {
        let runtime = IoRuntime::new().unwrap();
        let finished = Arc::new(AtomicBool::new(false));
        runtime
            .spawn({
                let finished = finished.clone();
                async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    finished.store(true, Ordering::SeqCst);
                    Ok(())
                }
            })
            .detach();
        std::thread::sleep(Duration::from_millis(300));
        assert!(finished.load(Ordering::SeqCst));
    }

    #[test]
    fn a_forwarded_stream_delivers_every_item_in_order() {
        let runtime = IoRuntime::new().unwrap();
        let stream = runtime.forward(futures::stream::iter(0..100), 4);
        let items: Vec<i32> = futures::executor::block_on(stream.collect());
        assert_eq!(items, (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn dropping_a_forwarded_stream_drops_the_source_on_the_runtime() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let runtime = IoRuntime::new().unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Guard(dropped.clone());
        let source = futures::stream::repeat(1).map(move |n| {
            let _ = &guard;
            n
        });
        let mut stream = runtime.forward(source, 1);
        assert_eq!(futures::executor::block_on(stream.next()), Some(1));
        drop(stream);
        std::thread::sleep(Duration::from_millis(200));
        assert!(dropped.load(Ordering::SeqCst));
    }
}
