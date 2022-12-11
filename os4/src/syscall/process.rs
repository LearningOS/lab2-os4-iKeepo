use crate::config::{MAX_SYSCALL_NUM};
use crate::task::{
    exit_current_and_run_next, suspend_current_and_run_next, TaskStatus, 
    get_status_of_current_task, get_syscall_times_of_current_task, get_start_time_of_current_task,
    get_phyaddress_from_current_task, mmap, munmap
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

#[derive(Clone, Copy)]
pub struct TaskInfo {
    pub status: TaskStatus,
    pub syscall_times: [u32; MAX_SYSCALL_NUM],
    pub time: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    info!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

// YOUR JOB: 引入虚地址后重写 sys_get_time
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    let us = get_time_us();
    
    let ts_tmp = get_phyaddress_from_current_task(ts as usize);
    
    let ts = ts_tmp as *mut TimeVal;
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

// CLUE: 从 ch4 开始不再对调度算法进行测试~
pub fn sys_set_priority(_prio: isize) -> isize {
    -1
}

// YOUR JOB: 扩展内核以实现 sys_mmap 和 sys_munmap
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    // MapArea::new(
    //     TRAP_CONTEXT.into(),
    //     TRAMPOLINE.into(),
    //     MapType::Framed,
    //     MapPermission::R | MapPermission::W,
    // );
    // MapArea::new(start_va, end_va, MapType::Framed, map_perm);
    mmap(start, len, port)
}

pub fn sys_munmap(start: usize, len: usize) -> isize {
    munmap(start, len)
}

// YOUR JOB: 引入虚地址后重写 sys_task_info
pub fn sys_task_info(ti: *mut TaskInfo) -> isize {
    let ts_tmp = get_phyaddress_from_current_task(ti as usize);
    
    let ti = ts_tmp as *mut TaskInfo;
    unsafe {
        *ti = TaskInfo{
            status: get_status_of_current_task(),
            syscall_times: get_syscall_times_of_current_task(),
            time: (get_time_us() - get_start_time_of_current_task()) / 1_000,
        }
    }
    0
}