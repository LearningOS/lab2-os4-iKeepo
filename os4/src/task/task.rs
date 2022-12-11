use super::TaskContext;
use crate::config::{kernel_stack_position, TRAP_CONTEXT, MAX_SYSCALL_NUM};
use crate::mm::{MapPermission, MemorySet, PhysPageNum, VirtAddr, KERNEL_SPACE};
use crate::trap::{trap_handler, TrapContext};

/// task control block structure
pub struct TaskControlBlock {
    pub task_status: TaskStatus,
    pub task_cx: TaskContext,
    pub memory_set: MemorySet,
    pub trap_cx_ppn: PhysPageNum, // trapcontext对应的物理页的页号（应用空间）
    pub base_size: usize, // user stack的栈顶

    pub syscall_times: [u32; MAX_SYSCALL_NUM],
    pub start_time: usize,
}

impl TaskControlBlock {
    /// 获得当前TaskControlBock中的TrapContext的可变引用
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }

    pub fn new(elf_data: &[u8], app_id: usize) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT).into())
            .unwrap()
            .ppn(); // 获得trapcontext对应的物理页的页号
        let task_status = TaskStatus::Ready;
        // map a kernel-stack in kernel space （虚拟地址空间）
        let (kernel_stack_bottom, kernel_stack_top) = kernel_stack_position(app_id);
        KERNEL_SPACE.lock().insert_framed_area(
            kernel_stack_bottom.into(),
            kernel_stack_top.into(),
            MapPermission::R | MapPermission::W,
        );
        let task_control_block = Self {
            task_status,
            task_cx: TaskContext::goto_trap_return(kernel_stack_top),
            memory_set,
            trap_cx_ppn,
            base_size: user_sp,

            syscall_times: [0 as u32; MAX_SYSCALL_NUM],
            start_time: 0 as usize,
        };
        // prepare TrapContext in user space
        // 注意：本函数第一行代码中创建memory_set的过程中并没有初始化TrapContext对应的物理页，这里就是初始化一下
        let trap_cx = task_control_block.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point, //这个入口不是在用户空间 依然在OS的”.data“中
            user_sp,
            KERNEL_SPACE.lock().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }
}

#[derive(Copy, Clone, PartialEq)]
/// task status: UnInit, Ready, Running, Exited
pub enum TaskStatus {
    UnInit,
    Ready,
    Running,
    Exited,
}