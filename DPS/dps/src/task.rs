use std::{
    future::Future,
    sync::{Arc, Mutex, MutexGuard},
};

pub struct Task<T: Send + 'static>(Arc<Mutex<Option<T>>>);

impl<T: Send + 'static> Clone for Task<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: Send + 'static> Task<T> {
    pub fn new(future: impl Future<Output = T> + Send + 'static) -> Self {
        let arc = Arc::new(Mutex::new(None));
        {
            let arc = Arc::clone(&arc);
            spawn(async move {
                let result = future.await;
                *arc.lock().unwrap() = Some(result);
            });
        }
        Self(arc)
    }

    pub fn pending() -> Self {
        Self::new(std::future::pending())
    }

    pub fn ok(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }

    pub fn take(&mut self) -> Option<T> {
        self.0.lock().unwrap().take()
    }

    pub fn get(&self) -> MutexGuard<'_, Option<T>> {
        self.0.lock().unwrap()
    }
}

impl<T: Send + Clone + 'static> Task<T> {
    pub fn clone_result(&self) -> Option<T> {
        self.0.lock().unwrap().clone()
    }
}

/// wasm 里没有 tokio 运行时，任务交给浏览器的微任务队列；桌面端仍用 tokio。
#[cfg(target_arch = "wasm32")]
fn spawn(future: impl Future<Output = ()> + 'static) {
    wasm_bindgen_futures::spawn_local(future);
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn(future: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(future);
}
