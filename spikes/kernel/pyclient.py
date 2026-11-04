import time, jupyter_client
km, kc = jupyter_client.manager.start_new_kernel(kernel_name="python3")
for n in [1_000_000, 4_000_000, 16_000_000, 50_000_000]:
    t=time.time(); got=0
    msg_id = kc.execute(f"print('x' * {n})")
    while True:
        m = kc.get_iopub_msg(timeout=120)
        if m["parent_header"].get("msg_id") != msg_id: continue
        if m["msg_type"] == "stream": got += len(m["content"]["text"])
        if m["msg_type"] == "status" and m["content"]["execution_state"] == "idle": break
    print(f"python 客户端 {n/1e6:.0f} MB → 收到 {got/1e6:.1f} MB，{time.time()-t:.2f} 秒")
km.shutdown_kernel()
