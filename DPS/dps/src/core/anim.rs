use super::{StaticTween, TweenFunction, TweenId, Tweenable, Vector};
use std::rc::Rc;

#[derive(Clone)]
pub struct Keyframe<T> {
    pub time: f64,
    pub value: T,
    pub tween: Rc<dyn TweenFunction>,
}

impl<T> Keyframe<T> {
    pub fn new(time: f64, value: T, tween: TweenId) -> Self {
        Self {
            time,
            value,
            tween: StaticTween::get_rc(tween),
        }
    }
}

#[derive(Clone)]
pub struct Anim<T: Tweenable> {
    pub time: f64,
    pub keyframes: Box<[Keyframe<T>]>,
    pub cursor: usize,
    pub next: Option<Box<Anim<T>>>,
}

impl<T: Tweenable> Default for Anim<T> {
    fn default() -> Self {
        Self {
            time: 0.0,
            keyframes: [].into(),
            cursor: 0,
            next: None,
        }
    }
}

impl<T: Tweenable> Anim<T> {
    pub fn new(keyframes: Vec<Keyframe<T>>) -> Self {
        // assert!(!keyframes.is_empty());
        // assert_eq!(keyframes[0].time, 0.0);
        // assert_eq!(keyframes.last().unwrap().tween, 0);
        Self {
            keyframes: keyframes.into_boxed_slice(),
            time: 0.0,
            cursor: 0,
            next: None,
        }
    }

    pub fn fixed(value: T) -> Self {
        Self {
            keyframes: Box::new([Keyframe::new(0.0, value, 0)]),
            time: 0.0,
            cursor: 0,
            next: None,
        }
    }

    pub fn is_default(&self) -> bool {
        self.keyframes.is_empty() && self.next.is_none()
    }

    /// 把多段动画首尾串起来：后一段的值叠加到前一段上。
    ///
    /// 空动画（没有关键帧、也没有后续段）在这里直接丢掉。它的 `now_opt` 是 `None`，
    /// 留在链上会让前一段的叠加拿到 `None`；谱面里 `"alphaEvents": []` 这类空事件数组
    /// 就会解析出这种动画。
    pub fn chain(elements: Vec<Anim<T>>) -> Self {
        let mut elements: Vec<_> = elements.into_iter().filter(|it| !it.is_default()).map(Box::new).collect();
        if elements.is_empty() {
            return Self::default();
        }
        elements.last_mut().unwrap().next = None;
        while elements.len() > 1 {
            let last = elements.pop().unwrap();
            elements.last_mut().unwrap().next = Some(last);
        }
        *elements.into_iter().next().unwrap()
    }

    pub fn dead(&self) -> bool {
        self.cursor + 1 >= self.keyframes.len()
    }

    pub fn set_time(&mut self, time: f64) {
        if self.keyframes.is_empty() || time == self.time {
            self.time = time;
            return;
        }
        while let Some(kf) = self.keyframes.get(self.cursor + 1) {
            if kf.time > time {
                break;
            }
            self.cursor += 1;
        }
        while self.cursor != 0 && self.keyframes[self.cursor].time > time {
            self.cursor -= 1;
        }
        self.time = time;
        if let Some(next) = &mut self.next {
            next.set_time(time);
        }
    }

    fn now_opt_inner(&self) -> Option<T> {
        if self.keyframes.is_empty() {
            return None;
        }
        Some(if self.cursor == self.keyframes.len() - 1 {
            self.keyframes[self.cursor].value.clone()
        } else {
            let kf1 = &self.keyframes[self.cursor];
            let kf2 = &self.keyframes[self.cursor + 1];
            let t = (self.time - kf1.time) / (kf2.time - kf1.time);
            T::tween(&kf1.value, &kf2.value, kf1.tween.y(t as f32))
        })
    }

    pub fn now_opt(&self) -> Option<T> {
        let Some(now) = self.now_opt_inner() else {
            return None;
        };
        Some(match &self.next {
            // 后续段可能是空动画（见 [`Self::chain`]）：它不贡献任何值，不能当成有值来叠加。
            Some(next) => match next.now_opt() {
                Some(value) => T::add(&now, &value),
                None => now,
            },
            None => now,
        })
    }

    pub fn map_value(&mut self, mut f: impl FnMut(T) -> T) {
        self.keyframes.iter_mut().for_each(|it| it.value = f(it.value.clone()));
        if let Some(next) = &mut self.next {
            next.map_value(f);
        }
    }
}

impl<T: Tweenable + Default> Anim<T> {
    pub fn now(&self) -> T {
        self.now_opt().unwrap_or_default()
    }
}

pub type AnimFloat = Anim<f32>;
pub type AnimFloatF64 = Anim<f64>;
#[derive(Clone, Default)]
pub struct AnimVector(pub AnimFloat, pub AnimFloat);

impl AnimVector {
    pub fn fixed(v: Vector) -> Self {
        Self(AnimFloat::fixed(v.x), AnimFloat::fixed(v.y))
    }

    pub fn set_time(&mut self, time: f64) {
        self.0.set_time(time);
        self.1.set_time(time);
    }

    pub fn now(&self) -> Vector {
        Vector::new(self.0.now(), self.1.now())
    }

    pub fn now_with_def(&self, x: f32, y: f32) -> Vector {
        Vector::new(self.0.now_opt().unwrap_or(x), self.1.now_opt().unwrap_or(y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anim(keyframes: &[(f64, f32)]) -> AnimFloat {
        // 2 号缓动是线性（0 号是常量 0，1 号是常量 1）。
        AnimFloat::new(keyframes.iter().map(|&(time, value)| Keyframe::new(time, value, 2)).collect())
    }

    /// 空事件数组（`"alphaEvents": []`）解析出来的是空动画：它挂在链尾时，前一段的叠加
    /// 不能因为拿到 `None` 而 panic。
    #[test]
    fn empty_layer_does_not_break_the_chain() {
        let mut chain = AnimFloat::chain(vec![anim(&[(0., 1.), (1., 2.)]), AnimFloat::default()]);
        chain.set_time(0.5);
        assert_eq!(chain.now_opt(), Some(1.5));

        // 空动画在链首时同样不参与叠加。
        let mut trailing = AnimFloat::chain(vec![AnimFloat::default(), anim(&[(0., 3.), (1., 3.)])]);
        trailing.set_time(0.5);
        assert_eq!(trailing.now_opt(), Some(3.));

        // 全是空动画时退化成 `default`，没有值。
        assert!(AnimFloat::chain(vec![AnimFloat::default(), AnimFloat::default()]).now_opt().is_none());
    }
}
