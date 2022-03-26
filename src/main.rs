#![feature(async_closure)]
#![allow(dead_code)]
mod timer_future;

use futures::{future::BoxFuture, stream::FuturesUnordered, task::ArcWake, FutureExt, StreamExt};
use lockfree::channel::mpmc::{self, Receiver, RecvErr, Sender};
use std::pin::Pin;
use std::sync::atomic::AtomicPtr;
use std::{future::Future, task::Context};
use std::{sync::Arc, time::Duration};
use timer_future::TimerFuture;

struct Executor {
    ready_queue: Receiver<Arc<Task>>,
}

#[derive(Clone)]
struct Spawner {
    task_sender: Sender<Arc<Task>>,
}

pub type F<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

struct Task {
    fut: AtomicPtr<Option<BoxFuture<'static, ()>>>,
    task_sender: Sender<Arc<Task>>,
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Task {{ {:?} }}", self.task_sender)
    }
}

#[inline(always)]
fn new_executor_and_spawner() -> (Executor, Spawner) {
    let (task_sender, ready_queue) = mpmc::create();

    (Executor { ready_queue }, Spawner { task_sender })
}

impl Spawner {
    #[inline(always)]
    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let fut = Box::into_raw(Box::new(Some(future.boxed())));
        let task = Arc::new(Task {
            fut: AtomicPtr::new(fut),
            task_sender: self.task_sender.clone(),
        });
        self.task_sender.send(task).unwrap();
    }
}

impl ArcWake for Task {
    #[inline(always)]
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let cloned = arc_self.clone();
        arc_self.task_sender.send(cloned).unwrap();
    }
}

impl Executor {
    #[inline(always)]
    fn run(&self) {
        let threads = num_cpus::get();

        let handles = (0..threads).map(|_| {
            let ready_queue = self.ready_queue.clone();
            std::thread::spawn(move || loop {
                match ready_queue.recv() {
                    Ok(task) => {
                        // Take the future, and if it has not yet completed (is still Some),
                        // poll it in an attempt to complete it.
                        let fut =
                            unsafe { &mut *task.fut.load(std::sync::atomic::Ordering::Acquire) };
                        if let Some(mut fut) = fut.take() {
                            let waker = futures::task::waker_ref(&task);
                            let cx = &mut Context::from_waker(&*waker);

                            if fut.as_mut().poll(cx).is_pending() {
                                // If the future is still pending, put it back in the queue.
                                task.fut.store(
                                    Box::into_raw(Box::new(Some(fut))),
                                    std::sync::atomic::Ordering::Release,
                                );
                            }
                        }
                    }
                    Err(err) => match err {
                        RecvErr::NoMessage => continue,
                        RecvErr::NoSender => break,
                    },
                }
            })
        });

        for handle in handles {
            handle.join().expect("thread panicked");
        }
    }
}

async fn say_hello() {
    println!("Hello, world!");
}

fn main() {
    let (executor, spawner) = new_executor_and_spawner();
    // Spawn a task to print before and after waiting on a timer.
    spawner.spawn(async {
        println!("howdy!");
        // Wait for our timer future to complete after two seconds.
        TimerFuture::new(Duration::new(2, 0)).await;
        println!("done!");

        (0..1 << 20)
            .map(|_| say_hello())
            .collect::<FuturesUnordered<_>>()
            .for_each(async move |_| {})
            .await;
    });

    // Drop the spawner so that our executor knows it is finished and won't
    // receive more incoming tasks to run.
    drop(spawner);

    // Run the executor until it is done.
    executor.run();
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    #[test]
    fn test_executor() {
        let (executor, spawner) = new_executor_and_spawner();
        spawner.spawn(async {
            println!("howdy!");
            TimerFuture::new(Duration::new(2, 0)).await;
            println!("done!");
        });

        drop(spawner);
        executor.run();
    }

    #[test]
    fn test_futs_unordered() {
        let (executor, spawner) = new_executor_and_spawner();
        spawner.spawn(async {
            (0..1 << 20)
                .map(|_| say_hello())
                .collect::<FuturesUnordered<_>>()
                .for_each(async move |_| {})
                .await;
        });

        drop(spawner);
        executor.run();
    }
}
