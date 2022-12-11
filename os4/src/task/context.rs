use crate::trap::trap_return;

#[derive(Copy, Clone)]
#[repr(C)]
/// task context structure containing some registers
pub struct TaskContext {
    ra: usize,
    sp: usize,
    s: [usize; 12],
}

impl TaskContext {

    /// 只用了一次，就是第一次调用app运行的时候
    pub fn zero_init() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s: [0; 12],
        }
    }
    /// 唯一经常使用的初始化方法
    pub fn goto_trap_return(kstack_ptr: usize) -> Self {
        Self {
            ra: trap_return as usize,
            sp: kstack_ptr,
            s: [0; 12],
        }
    }
}