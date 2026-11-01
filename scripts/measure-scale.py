#!/usr/bin/env python3
"""Reproducible native many-pane measurements (Linux/X11 + inspection build).

Frame CPU duration and command-to-file latency are reported separately from
parser throughput. Each case owns its state, endpoint and process. Results are evidence,
not a universal performance claim; run an optimized inspection build for timing.

`--keep-sessions on` runs every pane in this case's own session daemon (found by
its exact `--session-daemon <data root>/sessions` arguments) and reports its CPU,
RSS and context switches beside Neptune's. Those cases end through "End all
sessions and quit", then wait for the daemon to exit by itself.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import json
import hashlib
import os
from pathlib import Path
import platform
import shlex
import subprocess
import sys
import time
import uuid
import importlib.util
import math

REPO=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location("neptune_native_harness",REPO/"scripts/native-harness.py")
harness=importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)

def percentile(values, percentage):
    ordered=sorted(values)
    return ordered[max(0,min(len(ordered)-1,math.ceil(len(ordered)*percentage)-1))] if ordered else None

def tree_nodes(client,address):
    result=harness.inspect(client,address,"tree")["Tree"]["accesskit"]
    return [node for _,node in result["nodes"]]

def pane_nodes(client,address):
    return [node for node in tree_nodes(client,address) if node["properties"].get("label","").startswith("Terminal pane ")]

def click(client,address,node):
    b=node["properties"]["bounds"]
    harness.inspect(client,address,"click",str((b["x0"]+b["x1"])/2),str((b["y0"]+b["y1"])/2))

def frames(log):
    found=[]
    for line in log.read_text(errors="replace").splitlines():
        try: event=json.loads(line)
        except ValueError: continue
        if event.get("operation")=="frames": found.append(event)
    return found

def until(condition,timeout,label):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        value=condition()
        if value:return value
        time.sleep(.025)
    raise RuntimeError(f"Timed out waiting for {label}")

def save_layout(panes,split_counter,depth=0):
    if len(panes)==1:return {"kind":"pane","pane":panes[0]["id"]}
    split_counter[0]+=1;ident=split_counter[0];middle=len(panes)//2
    return {"kind":"split","id":ident,"axis":"vertical" if depth%2==0 else "horizontal","ratio":.5,"first":save_layout(panes[:middle],split_counter,depth+1),"second":save_layout(panes[middle:],split_counter,depth+1)}

def make_state(data,count):
    workspaces=[];counter=[0]
    for first in range(1,count+1,8):
        panes=[{"id":id,"cwd":str(data)} for id in range(first,min(first+8,count+1))]
        workspaces.append({"id":len(workspaces)+1,"name":f"Scale {len(workspaces)+1}","cwd":str(data),"panes":panes,"active":panes[-1]["id"],"layout":save_layout(panes,counter)})
    return {"version":1,"active":1,"sidebar":True,"workspaces":workspaces}

def command(client,address,text):
    harness.inspect(client,address,"key","ctrl+u")
    harness.inspect(client,address,"text",text)
    harness.inspect(client,address,"key","Enter")

def start_output(client,address,duration,marker):
    # Bound producer lifetime even if cleanup must forcibly stop the app.
    program=f"import os,time,pathlib;pathlib.Path({str(marker)!r}).write_text(str(os.getpid()));end=time.monotonic()+{duration!r};data=b'SCALE_OUTPUT '+b'x'*100+b'\\n';\nwhile time.monotonic()<end:\n os.write(1,data*16);time.sleep(.015)"
    command(client,address,f"python3 -c {shlex.quote(program)} &")
    until(marker.exists,10,"output producer startup marker")

def context_switches(pid):
    """Voluntary and involuntary switches summed over the process's live threads.

    A voluntary switch is a thread blocking, so its rate while quiet counts wakeups.
    Threads that exit during a sample take their counts with them."""
    total=0
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:status=(task/"status").read_text()
        except OSError:continue
        total+=sum(int(line.split()[1]) for line in status.splitlines() if line.startswith(("voluntary_ctxt_switches:","nonvoluntary_ctxt_switches:")))
    return total

def proc_sample(pid):
    status=Path(f"/proc/{pid}/status").read_text()
    stat=Path(f"/proc/{pid}/stat").read_text().split(') ',1)[1].split()
    rss=next(int(line.split()[1])*1024 for line in status.splitlines() if line.startswith("VmRSS:"))
    return {"cpu_ticks":int(stat[11])+int(stat[12]),"rss_bytes":rss,"threads":next(int(line.split()[1]) for line in status.splitlines() if line.startswith("Threads:")),"context_switches":context_switches(pid),"start_ticks":int(stat[19])}

def session_daemons(data,proc=Path("/proc")):
    """PIDs of the session daemons serving this data root, by exact arguments."""
    wanted=["--session-daemon",str(data/"sessions")];found=[]
    for entry in proc.iterdir():
        if not entry.name.isdigit():continue
        try:arguments=(entry/"cmdline").read_bytes().split(b"\0")[:-1]
        except OSError:continue
        if [argument.decode(errors="replace") for argument in arguments[1:]]==wanted:found.append(int(entry.name))
    return sorted(found)

def usage(before,after,elapsed):
    """CPU percent of one core, context switches per second and final RSS between two samples."""
    if before["start_ticks"]!=after["start_ticks"]:raise RuntimeError("A measured process was replaced during the sample")
    return {"cpu_core_percent":100*(after["cpu_ticks"]-before["cpu_ticks"])/os.sysconf("SC_CLK_TCK")/elapsed,"context_switches_per_second":(after["context_switches"]-before["context_switches"])/elapsed,"rss_bytes":after["rss_bytes"],"rss_delta_bytes":after["rss_bytes"]-before["rss_bytes"],"threads":after["threads"]}

def combined(processes):
    """Sum of Neptune's and its daemon's usage, for comparing with Neptune alone."""
    return {field:sum(process[field] for process in processes) for field in ("cpu_core_percent","context_switches_per_second","rss_bytes","threads")}

def node_labelled(client,address,label):
    return next((node for node in tree_nodes(client,address) if node["properties"].get("label")==label),None)

def end_all_sessions(client,address,process):
    """Quit through "End all sessions and quit", confirming the running-process warning."""
    harness.inspect(client,address,"key","ctrl+shift+p");time.sleep(.4)
    harness.inspect(client,address,"text","End all sessions and quit");time.sleep(.4)
    harness.inspect(client,address,"key","Enter")
    deadline=time.monotonic()+10
    while process.poll() is None and time.monotonic()<deadline:
        try:button=node_labelled(client,address,"End all and quit")
        except (OSError,RuntimeError,subprocess.TimeoutExpired):button=None
        if button:click(client,address,button)
        time.sleep(.2)
    process.wait(timeout=8)

def alive(pid,start_ticks):
    try:return int(Path(f"/proc/{pid}/stat").read_text().split(') ',1)[1].split()[19])==start_ticks
    except (OSError,IndexError,ValueError):return False

def stop_producers(data):
    """Stop any output producer of this case still running after teardown, by its recorded PID."""
    left=[]
    for marker in data.glob("producer-*"):
        try:pid=int(marker.read_text())
        except ValueError:continue
        try:arguments=Path(f"/proc/{pid}/cmdline").read_bytes()
        except OSError:continue
        if str(marker).encode() in arguments:
            left.append(pid);os.kill(pid,15)
    return left

def throughput(client,address,data,log,pane,mib):
    """Unthrottled output into one visible pane. The PTY backpressures the producer,
    so its own duration is the end-to-end rate; the parse time has the diagnostics'
    one-second resolution."""
    ident=int(pane["properties"]["label"].rsplit(' ',1)[1])
    prior=next(item["parsed"] for item in frames(log)[-1]["panes"] if item["pane"]==ident)
    line=b"THROUGHPUT "+b"y"*100+b"\r\n";count=mib*2**20//len(line);expected=count*len(line)
    times=data/"throughput-times"
    program=("import os,time,pathlib;"
             f"line={line!r};block=line*256;n={count};"
             "start=time.monotonic()\n"
             "for _ in range(n//256):\n view=memoryview(block)\n while view:view=view[os.write(1,view):]\n"
             "rest=memoryview(line*(n%256))\n"
             "while rest:rest=rest[os.write(1,rest):]\n"
             f"pathlib.Path({str(times)!r}).write_text(f'{{start}} {{time.monotonic()}}')")
    click(client,address,pane)
    command(client,address,"stty -echo -onlcr; python3 -c "+shlex.quote(program)+"; stty echo onlcr")
    done=until(lambda:times.exists() and times.read_text(),600,"throughput producer")
    start,end=map(float,done.split())
    until(lambda:any(item["pane"]==ident and item["parsed"]>=prior+expected and item["parsed"]==item["received"] for item in frames(log)[-1]["panes"]),120,"all throughput output parsed")
    parsed_at=time.monotonic()
    return {"bytes":expected,"producer_seconds":end-start,"producer_mib_per_second":expected/2**20/(end-start),"parsed_seconds_upper_bound":parsed_at-start,"parsed_mib_per_second_lower_bound":expected/2**20/(parsed_at-start)}

ECHO_PROGRAM=r"""
import os,sys,termios,time,tty
log=open(sys.argv[1],'a',buffering=1);fd=sys.stdin.fileno();saved=termios.tcgetattr(fd);tty.setraw(fd);count=0
os.write(1,b']2;echo-ready')
try:
    while True:
        key=os.read(fd,1)
        if not key or key==b'q':break
        count+=1;log.write(f"{count} {time.monotonic()}
")
        os.write(1,b']2;echo-%d'%count+key)
finally:termios.tcsetattr(fd,termios.TCSADRAIN,saved)
"""

def title_shown(client,address,title):
    return any(title in (node["properties"].get("label","")+" "+str(node["properties"].get("value",""))) for node in tree_nodes(client,address))

def echo_delay(client,address,data,pane,samples):
    """Keystroke to echo: an inspection key reaches a raw-mode program, which answers
    by setting the terminal title; the delay ends when Neptune's accessibility tree
    shows that title. The program's own read time splits the input and output legs."""
    script=data/"echo.py";script.write_text(ECHO_PROGRAM);record=data/"echo.log"
    click(client,address,pane)
    command(client,address,f"python3 {shlex.quote(str(script))} {shlex.quote(str(record))}")
    until(lambda:title_shown(client,address,"echo-ready"),15,"echo program title")
    totals=[];inputs=[];outputs=[]
    for index in range(1,samples+1):
        time.sleep(.2)
        sent=time.monotonic();harness.inspect(client,address,"text","x")
        until(lambda:title_shown(client,address,f"echo-{index}"),10,"echo title")
        shown=time.monotonic()
        read=float(record.read_text().splitlines()[index-1].split()[1])
        totals.append((shown-sent)*1000);inputs.append((read-sent)*1000);outputs.append((shown-read)*1000)
    harness.inspect(client,address,"text","q")
    summary=lambda values:{"p50":percentile(values,.5),"p95":percentile(values,.95),"p99":percentile(values,.99)}
    return {"scope":"inspection text request to the terminal title shown in the accessibility tree; each poll is a separate inspection client run, not keyboard-to-screen","total_ms":summary(totals),"input_ms":summary(inputs),"output_ms":summary(outputs),"samples_ms":totals}

def run_case(args,count,scenario,root):
    output=root/f"{count}-{scenario}";data=output/"data";data.mkdir(parents=True)
    if scenario=="hidden-output" and count==1:
        result={"panes":count,"scenario":scenario,"status":"not-applicable","reason":"One pane has no independent hidden pane"}
        (output/"result.json").write_text(json.dumps(result,indent=2)+"\n");return result
    (data/"workspaces.json").write_text(json.dumps(make_state(data,count)))
    keep=args.keep_sessions=="on"
    (data/"config.toml").write_text(f'shell="/bin/sh"\nscrollback={args.scrollback}\ncursor_blink=false\nconfirm_close=false\nwarn_running_processes=false\nkeep_sessions_running={str(keep).lower()}\n')
    address=harness.free_endpoint();env=os.environ.copy();env.pop("WAYLAND_DISPLAY",None);env["EGUI_INSPECTION"]=address
    # A test instance launched from inside Neptune must not join that Neptune's agents.
    for name in [name for name in env if name.startswith("NEPTUNE_AGENT")]:env.pop(name)
    log=output/"app.log";result={"panes":count,"scenario":scenario,"keep_sessions":args.keep_sessions,"endpoint":address,"data_root":str(data),"status":"running","output_producers":[]}
    process=None;daemons=[];daemon_start={}
    try:
        with log.open('w') as sink:
            started=time.monotonic();process=subprocess.Popen([str(args.app),"--data-root",str(data),"--diagnostics","--size","1180x760"],env=env,stdout=sink,stderr=subprocess.STDOUT,start_new_session=True)
            harness.wait_ready(process,args.client,address,30)
            until(lambda:frames(log) and frames(log)[-1]["resources"]["running"]==count,45,"all pane startups")
            result["startup_seconds"]=time.monotonic()-started
            daemons=session_daemons(data);result["daemon_pids"]=daemons
            if len(daemons)!=(1 if keep else 0):raise RuntimeError(f"Expected {1 if keep else 0} session daemon for this data root, found {daemons}")
            daemon_start={pid:proc_sample(pid)["start_ticks"] for pid in daemons}
            panes=pane_nodes(args.client,address)
            # Keep every producer alive through eight sequential setups and
            # latency probes, while still bounding a forcibly interrupted run.
            lifetime=args.seconds+600
            def produce(pane):
                label=pane["properties"]["label"];ident=int(label.rsplit(' ',1)[1])
                prior=next(item["parsed"] for item in frames(log)[-1]["panes"] if item["pane"]==ident)
                click(args.client,address,pane)
                marker=data/f"producer-{ident}";start_output(args.client,address,lifetime,marker)
                until(lambda:any(item["pane"]==ident and item["parsed"]>=prior+10_000 for item in frames(log)[-1]["panes"]),10,"verified producer bytes")
                result["output_producers"].append(label)
            if scenario=="visible-output":
                produce(panes[-1])
            elif scenario=="several-output":
                for pane in panes:produce(pane)
            elif scenario=="hidden-output":
                produce(panes[-1])
                if count>8:harness.inspect(args.client,address,"key","ctrl+Tab")
                elif count>1:
                    click(args.client,address,panes[0]);harness.inspect(args.client,address,"key","ctrl+shift+Enter")
            elif scenario=="lifecycle":
                for _ in range(6):
                    if count==64:
                        harness.inspect(args.client,address,"key","ctrl+shift+w")
                        until(lambda:frames(log) and frames(log)[-1]["resources"]["running"]==count-1 and frames(log)[-1]["resources"]["closing"]==0,30,"closed slot teardown")
                        harness.inspect(args.client,address,"key","ctrl+shift+d")
                    else:
                        harness.inspect(args.client,address,"key","ctrl+shift+d")
                        until(lambda:frames(log) and frames(log)[-1]["resources"]["running"]==count+1 and frames(log)[-1]["resources"]["starting"]==0,30,"split startup")
                        harness.inspect(args.client,address,"key","ctrl+shift+w")
                    until(lambda:frames(log) and frames(log)[-1]["resources"]["running"]==count and frames(log)[-1]["resources"]["starting"]==0 and frames(log)[-1]["resources"]["closing"]==0,30,"rapid lifecycle cleanup")
            if scenario=="throughput":
                watched=[process.pid,*daemons];before={pid:proc_sample(pid) for pid in watched};tick=time.monotonic()
                result["throughput"]=throughput(args.client,address,data,log,panes[-1],args.throughput_mib)
                elapsed=time.monotonic()-tick;after={pid:proc_sample(pid) for pid in watched}
                result["neptune"]=usage(before[process.pid],after[process.pid],elapsed)
                result["daemons"]=[usage(before[pid],after[pid],elapsed) for pid in daemons]
                result["combined"]=combined([result["neptune"],*result["daemons"]])
            elif scenario=="echo":
                result["echo"]=echo_delay(args.client,address,data,panes[-1],args.echo_samples)
            latencies=[]
            # End-to-end latency is measured separately from internal frame cost.
            for index in range(5 if scenario not in ("throughput","echo") else 0):
                marker=data/f"latency-{index}";tick=time.monotonic()
                command(args.client,address,f"printf ready > {shlex.quote(str(marker))}")
                until(marker.exists,10,"shell latency marker");latencies.append((time.monotonic()-tick)*1000)
            # Probe traffic belongs outside the quiet CPU/RSS interval.
            time.sleep(.25)
            if scenario not in ("throughput","echo"):
                before_parsed={item["pane"]:item["parsed"] for item in frames(log)[-1]["panes"]}
                watched=[process.pid,*daemons];before={pid:proc_sample(pid) for pid in watched};measure_start=time.monotonic()
                time.sleep(args.seconds)
                elapsed=time.monotonic()-measure_start;after={pid:proc_sample(pid) for pid in watched};events=frames(log)
                after_parsed={item["pane"]:item["parsed"] for item in events[-1]["panes"]}
                activity={label:after_parsed[int(label.rsplit(' ',1)[1])]-before_parsed[int(label.rsplit(' ',1)[1])] for label in result["output_producers"]}
                result["output_activity_bytes"]=activity
                if any(delta<10_000 for delta in activity.values()):raise RuntimeError("An intended output producer stopped before the CPU/RSS sample completed")
                neptune=usage(before[process.pid],after[process.pid],elapsed);result["daemons"]=[usage(before[pid],after[pid],elapsed) for pid in daemons]
                result["combined"]=combined([neptune,*result["daemons"]])
                result.update({"sample_seconds":elapsed,"cpu_core_percent":neptune["cpu_core_percent"],"context_switches_per_second":neptune["context_switches_per_second"],"rss_bytes":neptune["rss_bytes"],"rss_delta_bytes":neptune["rss_delta_bytes"],"threads":neptune["threads"],"latency_scope":"inspection text/key request to shell file creation; includes inspection roundtrip and readiness polling, not keyboard-to-screen","latency_ms":{"p50":percentile(latencies,.5),"p95":percentile(latencies,.95),"p99":percentile(latencies,.99),"samples":latencies},"frames":events[-1] if events else None})
            harness.inspect(args.client,address,"screenshot",str(output/"native.png"))
            if keep:
                # Closing the window would only detach: end the sessions instead.
                end_all_sessions(args.client,address,process)
                until(lambda:not any(alive(pid,start) for pid,start in daemon_start.items()),30,"session daemon exit once empty")
                result["daemon_exited"]=True
            else:
                close=next(node for node in tree_nodes(args.client,address) if node["properties"].get("label")=="Close window")
                click(args.client,address,close);process.wait(timeout=8)
            shutdowns=[json.loads(line) for line in log.read_text(errors="replace").splitlines() if line.startswith('{') and '"operation":"shutdown"' in line]
            result["shutdown_complete"]=bool(shutdowns and shutdowns[-1]["complete"])
            if not result["shutdown_complete"]:raise RuntimeError("Session teardown did not complete within shutdown boundary")
            result["status"]="passed"
    except Exception as error:
        result["status"]="failed";result["error"]=str(error)
    finally:
        if process is not None:harness.stop_owned(process)
        left=stop_producers(data)
        if left:result["stopped_producers"]=left
        # A daemon this case started and could not end through the app.
        for pid,start in daemon_start.items():
            if alive(pid,start):os.kill(pid,15);result.setdefault("stopped_daemons",[]).append(pid)
        (output/"result.json").write_text(json.dumps(result,indent=2)+'\n')
    print(f"{count:2} panes {scenario}: {result['status']}",flush=True)
    return result

def main():
    parser=argparse.ArgumentParser(description=__doc__,formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--app',type=Path,default=REPO/'target/release/neptune')
    parser.add_argument('--client',type=Path,default=REPO/'target/release/neptune-inspect')
    parser.add_argument('--counts',type=int,nargs='+',default=[1,8,32,64])
    parser.add_argument('--scenarios',nargs='+',choices=['idle','visible-output','several-output','hidden-output','lifecycle','throughput','echo'],default=['idle','visible-output','several-output','hidden-output','lifecycle'])
    parser.add_argument('--seconds',type=float,default=5)
    parser.add_argument('--keep-sessions',choices=['off','on'],default='off',help='run the panes in a session daemon (on) or in Neptune (off)')
    parser.add_argument('--scrollback',type=int,default=1000,help='scrollback lines per pane (default: 1000)')
    parser.add_argument('--throughput-mib',type=int,default=256,help='output written by the throughput case (default: 256 MiB)')
    parser.add_argument('--echo-samples',type=int,default=40,help='keystrokes timed by the echo case (default: 40)')
    parser.add_argument('--output',type=Path,default=REPO/'artifacts/scale')
    args=parser.parse_args();args.app=args.app.resolve();args.client=args.client.resolve()
    if not sys.platform.startswith('linux'):parser.error('Process CPU/RSS adapter currently supports Linux only')
    if args.seconds<1 or args.seconds>60 or any(count<1 or count>64 for count in args.counts):parser.error('Use 1..64 panes and 1..60 sample seconds')
    if not args.app.is_file() or not args.client.is_file():parser.error('Build both release inspection binaries first')
    run=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-'+uuid.uuid4().hex[:8];root=args.output.resolve()/run;root.mkdir(parents=True)
    report={"run_id":run,"platform":platform.platform(),"compiler":subprocess.check_output(['rustc','--version'],text=True).strip(),"features":["inspection"],"app":str(args.app),"binary_sha256":hashlib.sha256(args.app.read_bytes()).hexdigest(),"lock_sha256":hashlib.sha256((REPO/"Cargo.lock").read_bytes()).hexdigest(),"script_sha256":hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),"sample_seconds":args.seconds,"keep_sessions":args.keep_sessions,"cases":[]}
    for count in args.counts:
        for scenario in args.scenarios:
            # Lifecycle at full capacity releases and restores the same slot.
            report['cases'].append(run_case(args,count,scenario,root));(root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    print(f"Measurements: {root/'report.json'}",flush=True)
    return int(any(case['status'] not in ('passed','not-applicable') for case in report['cases']))
if __name__=='__main__':raise SystemExit(main())
