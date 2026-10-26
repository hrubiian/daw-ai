#!/usr/bin/env python3
"""Render new offline studies through the existing daw-ai MCP server."""
import argparse
import hashlib
import json
import select
import shutil
import subprocess
import time
from pathlib import Path


def render(repo, session, destination, bar_range=None, tracks=None):
    binary=repo/'target/debug/daw-mcp'
    process=subprocess.Popen([str(binary),str(session)],cwd=repo,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
    counter=0
    def rpc(method, params):
        nonlocal counter
        counter+=1
        process.stdin.write(json.dumps(dict(jsonrpc='2.0',id=counter,method=method,params=params))+'\n');process.stdin.flush()
        if not select.select([process.stdout],[],[],45)[0]:raise TimeoutError(method)
        response=json.loads(process.stdout.readline())
        if 'error' in response:raise RuntimeError(response)
        r=response['result']
        if r.get('isError'):raise RuntimeError(r)
        return r
    def tool(name,args=None):
        r=rpc('tools/call',dict(name=name,arguments=args or {}))
        return '\n'.join(c.get('text','') for c in r.get('content',[]) if c['type']=='text')
    try:
        rpc('initialize',dict(protocolVersion='2024-11-05',capabilities={},clientInfo=dict(name='bonobo-study',version='1')))
        process.stdin.write(json.dumps(dict(jsonrpc='2.0',method='notifications/initialized'))+'\n');process.stdin.flush()
        summary=tool('project_summary')
        arguments={}
        if bar_range:arguments.update(start_bar=bar_range[0],end_bar=bar_range[1])
        if tracks:arguments['tracks']=tracks
        response=tool('start_listen',arguments).strip()
        job_id=response
        if not job_id:raise RuntimeError(response)
        deadline=time.monotonic()+240
        while time.monotonic()<deadline:
            status=json.loads(tool('job_status',dict(id=job_id)))
            state=status.get('status',status.get('state'))
            if state in ['completed','complete','succeeded','failed','cancelled']:break
            time.sleep(.4)
        else:raise TimeoutError('Render deadline')
        if state not in ['completed','complete','succeeded']:raise RuntimeError(status)
        # Persist full tool response, then find its recorded WAV artifact.
        destination.with_suffix('.job.json').write_text(json.dumps(dict(summary=summary,job=status,binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest()),indent=2)+'\n')
        def paths(x):
            if isinstance(x,dict):
                for v in x.values():yield from paths(v)
            elif isinstance(x,list):
                for v in x:yield from paths(v)
            elif isinstance(x,str) and x.endswith('.wav'):yield x
        candidates=[Path(x) for x in paths(status)]
        source=next((x for x in candidates if x.is_file()),None)
        if source is None:raise RuntimeError(f'No WAV artifact in {status}')
        shutil.copyfile(source,destination)
        print(destination,flush=True)
    finally:
        process.stdin.close()
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:process.kill();process.wait()


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--repo',required=True,type=Path)
    p.add_argument('--directory',required=True,type=Path)
    p.add_argument('--stem',action='append')
    p.add_argument('--range',nargs=2,type=int,dest='bar_range')
    p.add_argument('--tracks',nargs='+')
    a=p.parse_args(); directory=a.directory.resolve(); out=directory/'renders';out.mkdir(exist_ok=True)
    for stem in a.stem or ['estuary','estuary-loop','three-lanterns']:
        suffix='-bars-'+'-'.join(map(str,a.bar_range)) if a.bar_range else ''
        if a.tracks:suffix+='-'+'-'.join(a.tracks)
        render(a.repo.resolve(),directory/(stem+'.daw.json'),out/(stem+suffix+'.wav'),a.bar_range,a.tracks)


if __name__=='__main__':main()
