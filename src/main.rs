#![feature(async_closure)]
#![allow(dead_code)]
mod spin_lock;
mod timer_future;

use futures::{future::BoxFuture, stream::FuturesUnordered, task::ArcWake, FutureExt, StreamExt};
use lockfree::channel::mpmc::{self, Receiver, RecvErr, Sender};
use spin_lock::SpinLock;
use std::cell::UnsafeCell;
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

struct Task {
    fut: UnsafeCell<Option<BoxFuture<'static, ()>>>,
    pause: SpinLock,
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
        let fut = Some(future.boxed());
        let task = Arc::new(Task {
            fut: UnsafeCell::new(fut),
            pause: SpinLock::new(),
            task_sender: self.task_sender.clone(),
        });
        self.task_sender.send(task).expect("too many tasks queued");
    }
}

impl ArcWake for Task {
    #[inline(always)]
    fn wake_by_ref(arc_self: &Arc<Self>) {
        let cloned = arc_self.clone();
        arc_self
            .task_sender
            .send(cloned)
            .expect("too many tasks queued");
    }
}

impl Executor {
    #[inline(always)]
    fn run(&self) {
        loop {
            match self.ready_queue.recv() {
                Ok(task) => {
                    // Take the future, and if it has not yet completed (is still Some),
                    // poll it in an attempt to complete it.
                    task.pause.lock();
                    if let Some(fut) = unsafe { &mut *task.fut.get() } {
                        let waker = futures::task::waker_ref(&task);
                        let cx = &mut Context::from_waker(&*waker);

                        if fut.as_mut().poll(cx).is_ready() {
                            // The future has completed, so we can drop it.
                            unsafe {
                                *task.fut.get() = None;
                            }
                        }
                    }
                    task.pause.unlock();
                }
                Err(err) => match err {
                    RecvErr::NoMessage => continue,
                    RecvErr::NoSender => break,
                },
            }
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
