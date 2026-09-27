/*
 * Linux-Pilot 的最小 eBPF 探针。
 *
 * 用户态 Agent 使用 Aya 加载这个 ELF。此文件只记录计数和延迟直方图，
 * 不读取进程内存、网络载荷或文件内容。使用 tp_btf 的块设备探针需要
 * 宿主机提供对应 BTF；不可用时用户态会保留常规指标并标记能力缺失。
 */

typedef unsigned int u32;
typedef unsigned long long u64;

#define SEC(name) __attribute__((section(name), used))
#define __uint(name, value) int (*name)[value]
#define __type(name, value) value *name

/* Linux uapi bpf_map_type: HASH=1, ARRAY=2。 */
struct {
    __uint(type, 2);
    __uint(max_entries, 104);
    __type(key, u32);
    __type(value, u64);
} COUNTERS SEC(".maps");

struct {
    __uint(type, 1);
    __uint(max_entries, 8192);
    __type(key, u64);
    __type(value, u64);
} BLOCK_START SEC(".maps");

struct {
    __uint(type, 1);
    __uint(max_entries, 16384);
    __type(key, u64);
    __type(value, u64);
} CONNECT_START SEC(".maps");

struct {
    __uint(type, 1);
    __uint(max_entries, 16384);
    __type(key, u64);
    __type(value, u64);
} SCHED_START SEC(".maps");

/* eBPF helper ID 是内核 ABI，声明为函数指针供编译器生成 helper 调用。 */
static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)1;
static long (*bpf_map_update_elem)(void *map, const void *key, const void *value, u64 flags) = (void *)2;
static long (*bpf_map_delete_elem)(void *map, const void *key) = (void *)3;
static u64 (*bpf_ktime_get_ns)(void) = (void *)5;

static __attribute__((always_inline)) void increment(u32 key, u64 delta) {
    u64 *value = bpf_map_lookup_elem(&COUNTERS, &key);
    if (value) {
        /* 多核同时命中时需要原子累加，否则高频事件会丢计数。 */
        __sync_fetch_and_add(value, delta);
    }
}

static __attribute__((always_inline)) void observe(u32 first_bucket, u64 elapsed_ns) {
    u64 microseconds = elapsed_ns / 1000;
    u32 bucket = 0;
    while (microseconds > 1 && bucket < 31) {
        microseconds >>= 1;
        bucket++;
    }
    increment(first_bucket + bucket, 1);
}

SEC("kprobe/tcp_retransmit_skb")
int tcp_retrans(void *ctx) {
    (void)ctx;
    increment(0, 1);
    return 0;
}

SEC("tracepoint/sched/sched_switch")
int sched_switch(void *ctx) {
    (void)ctx;
    increment(1, 1);
    return 0;
}

SEC("tp_btf/sched_wakeup")
int sched_wakeup(u64 *ctx) {
    u64 task = ctx[0];
    u64 started = bpf_ktime_get_ns();
    bpf_map_update_elem(&SCHED_START, &task, &started, 0);
    return 0;
}

SEC("tp_btf/sched_switch")
int sched_wait_finish(u64 *ctx) {
    /* sched_switch(preempt, prev, next, prev_state)：第三个参数是 next。 */
    u64 task = ctx[2];
    u64 *started = bpf_map_lookup_elem(&SCHED_START, &task);
    if (!started) return 0;
    u64 elapsed_ns = bpf_ktime_get_ns() - *started;
    bpf_map_delete_elem(&SCHED_START, &task);
    /* 仅把超过 10ms 的等待计入“长等待”；所有等待仍进入直方图。 */
    if (elapsed_ns > 10000000ULL) increment(71, 1);
    observe(72, elapsed_ns);
    return 0;
}

SEC("tp_btf/inet_sock_set_state")
int tcp_state(u64 *ctx) {
    /* inet_sock_set_state(sk, oldstate, newstate)，2=SYN_SENT、1=ESTABLISHED、7=CLOSE。 */
    u64 socket = ctx[0];
    int oldstate = (int)ctx[1];
    int newstate = (int)ctx[2];
    if (newstate == 2) {
        u64 started = bpf_ktime_get_ns();
        bpf_map_update_elem(&CONNECT_START, &socket, &started, 0);
        increment(36, 1);
    } else if (oldstate == 2 && newstate == 1) {
        u64 *started = bpf_map_lookup_elem(&CONNECT_START, &socket);
        if (started) {
            observe(39, bpf_ktime_get_ns() - *started);
            bpf_map_delete_elem(&CONNECT_START, &socket);
        }
        increment(38, 1);
    } else if (oldstate == 2 && newstate == 7) {
        bpf_map_delete_elem(&CONNECT_START, &socket);
        increment(37, 1);
    }
    return 0;
}

SEC("tp_btf/block_rq_issue")
int block_issue(u64 *ctx) {
    u64 request = ctx[0];
    u64 started = bpf_ktime_get_ns();
    bpf_map_update_elem(&BLOCK_START, &request, &started, 0);
    return 0;
}

SEC("tp_btf/block_rq_complete")
int block_complete(u64 *ctx) {
    u64 request = ctx[0];
    u64 *started = bpf_map_lookup_elem(&BLOCK_START, &request);
    if (!started) return 0;
    u64 elapsed_ns = bpf_ktime_get_ns() - *started;
    bpf_map_delete_elem(&BLOCK_START, &request);
    increment(2, 1);
    increment(3, elapsed_ns);
    /* 以 2 的幂为边界保存微秒直方图，可在用户态求近似 p95。 */
    observe(4, elapsed_ns);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
