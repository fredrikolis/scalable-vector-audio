// Concern: runs independent tasks on at most a thread limit, answers in task order | Non-concern: which tasks are independent | IO: (limit, tasks) -> answers

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn default_threads() -> NonZeroUsize {
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

pub(crate) fn each<T: Send, R: Send>(
    limit: NonZeroUsize,
    tasks: Vec<T>,
    run: impl Fn(T) -> R + Sync,
) -> Vec<R> {
    let workers = limit.get().min(tasks.len());
    if workers <= 1 {
        return tasks.into_iter().map(run).collect();
    }
    let answers: Vec<Mutex<Option<R>>> = tasks.iter().map(|_| Mutex::new(None)).collect();
    let tasks: Vec<Mutex<Option<T>>> = tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            let at = next.fetch_add(1, Ordering::Relaxed);
            let Some(task) = tasks.get(at) else {
                return;
            };
            let task = task.lock().expect("a task").take().expect("taken once");
            *answers[at].lock().expect("an answer") = Some(run(task));
        }
    };
    rayon::scope(|scope| {
        for _ in 1..workers {
            scope.spawn(|_| work());
        }
        work();
    });
    answers
        .into_iter()
        .map(|answer| {
            answer
                .into_inner()
                .expect("an answer")
                .expect("each task ran")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn tasks_run_on_at_most_the_limit_and_answer_in_task_order() {
        let (running, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let limit = std::num::NonZeroUsize::new(3).expect("three");
        let answers = super::each(limit, (0..64u64).collect(), |task| {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            let spun = (0..10_000u64).fold(task, |held, k| std::hint::black_box(held ^ k));
            running.fetch_sub(1, Ordering::SeqCst);
            (task, spun)
        });
        let tasks: Vec<u64> = answers.iter().map(|(task, _)| *task).collect();
        assert_eq!(tasks, (0..64).collect::<Vec<_>>());
        assert!(most.load(Ordering::SeqCst) <= 3, "{most:?} at once");
    }
}
