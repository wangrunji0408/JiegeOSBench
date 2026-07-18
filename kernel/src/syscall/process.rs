pub fn sys_exit(exit_code: i32) -> isize {
    crate::println!(
        "[kernel] pid={} exited with code {}",
        crate::task::current_pid(),
        exit_code
    );
    crate::task::exit_current_and_run_next(exit_code);
}
