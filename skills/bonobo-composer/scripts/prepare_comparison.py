#!/usr/bin/env python3
"""Prepare gain-only listening masters and matched excerpts; no quality ratings."""
import argparse
import html
import json
import re
import subprocess
from pathlib import Path


def measure(path):
    r=subprocess.run(['ffmpeg','-hide_banner','-nostats','-i',str(path),'-af','loudnorm=I=-20:TP=-2:LRA=11:print_format=json','-f','null','-'],capture_output=True,text=True,check=True)
    match=re.findall(r'\{\s*"input_i".*?\}',r.stderr,re.S)
    if not match:raise RuntimeError(r.stderr[-3000:])
    return {k:float(v) for k,v in json.loads(match[-1]).items() if k.startswith('input_')}


def ffmpeg(source,dest,filters=None,start=None,seconds=None):
    cmd=['ffmpeg','-v','error','-y','-i',str(source)]
    if start is not None:cmd+=['-ss',str(start)]
    if seconds is not None:cmd+=['-t',str(seconds)]
    if filters:cmd+=['-af',filters]
    cmd+=['-c:a','libmp3lame','-q:a','2'] if dest.suffix=='.mp3' else ['-c:a','pcm_s24le']
    subprocess.run(cmd+[str(dest)],check=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--experiment',required=True,type=Path)
    a=parser.parse_args();root=a.experiment.resolve();studies=root/'studies-v2'
    delivery=root/'delivery';delivery.mkdir(exist_ok=True)
    clips=delivery/'comparison';clips.mkdir(exist_ok=True)
    settings={'processing':'Gain and short fades only; no compression, EQ or limiting.', 'masters':{},'excerpts':{},'listener_ratings':None}
    titles={'estuary':'Estuary','estuary-loop':'Estuary — repeated-loop version','three-lanterns':'Three Lanterns'}
    for stem,title in titles.items():
        source=studies/'renders'/(stem+'.wav');stats=measure(source)
        duration=float(subprocess.check_output(['ffprobe','-v','error','-show_entries','format=duration','-of','csv=p=0',str(source)]))
        gain=min(-18-stats['input_i'],-2.5-stats['input_tp'])
        filters=f'volume={gain:.5f}dB,afade=t=in:d=0.015,afade=t=out:st={duration-2.0:.5f}:d=2.0'
        wav=delivery/(stem+'.wav');ffmpeg(source,wav,filters)
        mp3=delivery/(stem+'.mp3');ffmpeg(wav,mp3)
        mp3stats=measure(mp3)
        if mp3stats['input_tp']>-1.0:
            ffmpeg(wav,mp3,f'volume={-1.5-mp3stats["input_tp"]:.5f}dB');mp3stats=measure(mp3)
        settings['masters'][stem]=dict(title=title,input=stats,gain_db=gain,output_wav=measure(wav),output_mp3=mp3stats,duration=duration)
    # Same-clock-window source selections; these are NOT certified equivalent
    # song sections because reference-preview offsets are unknown.
    # Start at bar 20 so A/B includes the bar-24 transition into the breakdown;
    # an excerpt wholly inside one loop cannot test that transition's fluency.
    sources=[('A',studies/'renders/estuary-loop.wav',20*240/98),('B',studies/'renders/estuary.wav',20*240/98),('Kiara',root/'references/kiara.m4a',10),('Cirrus',root/'references/cirrus.m4a',10),('Black Sands',root/'references/black-sands.m4a',10),('Three Lanterns',studies/'renders/three-lanterns.wav',16)]
    for label,path,start in sources:
        stem=label.lower().replace(' ','-');wav=clips/(stem+'-source.wav')
        ffmpeg(path,wav,start=start,seconds=19)
        settings['excerpts'][label]=dict(source=str(path),start_seconds=start,seconds=19,input=measure(wav),temporary=wav.name)
    target=min(-20,min(v['input']['input_i']-2-v['input']['input_tp'] for v in settings['excerpts'].values()))
    for label,v in settings['excerpts'].items():
        gain=target-v['input']['input_i'];wav=clips/(label.lower().replace(' ','-')+'.wav')
        ffmpeg(clips/v['temporary'],wav,f'volume={gain:.5f}dB')
        v.update(gain_db=gain,output=measure(wav),file=wav.name)
        (clips/v.pop('temporary')).unlink()
    settings['comparison_target_lufs']=target
    settings['comparison_limit']='19-second excerpts; same clock-window selection, unknown preview offsets; musical function not validated by listening.'
    (root/'delivery-settings.json').write_text(json.dumps(settings,indent=2)+'\n')
    (root/'ab-key.json').write_text(json.dumps({'A':'Repeated-loop arrangement, v2 balance','B':'Developed arrangement, v2 balance','scope':'label masking only; same author, no controlled no-skill trial'},indent=2)+'\n')
    ratings={name:{key:None for key in ['groove','fluency','development','timbre_depth','mix','overall','comment']} for name in settings['excerpts']}
    (root/'listener-ratings.json').write_text(json.dumps(ratings,indent=2)+'\n')
    cards=[]
    for label,v in settings['excerpts'].items():
        fields=''.join(f'<label>{title}<select data-dimension="{key}"><option value="">Unrated</option>'+''.join(f'<option>{i}</option>' for i in range(1,6))+'</select></label>' for key,title in [('groove','Groove'),('fluency','Fluency'),('development','Development'),('timbre_depth','Timbre / depth'),('mix','Mix'),('overall','Overall')])
        cards.append(f'<article data-label="{html.escape(label)}"><h2>{html.escape(label)}</h2><audio controls preload="none" src="delivery/comparison/{v["file"]}"></audio><div class="ratings">{fields}</div><textarea placeholder="One concrete listening observation" aria-label="Listening observation"></textarea><small>{v["output"]["input_i"]:.1f} LUFS · {v["output"]["input_tp"]:.1f} dBTP</small></article>')
    masters=''.join(f'<p><b>{html.escape(title)}</b><br><audio controls preload="none" src="delivery/{stem}.mp3"></audio></p>' for stem,title in titles.items() if stem!='estuary-loop')
    page='''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Composer experiment · Listening desk</title><style>
    body{font:17px/1.55 system-ui,sans-serif;background:#101b21;color:#e2ecee;max-width:1100px;margin:40px auto;padding:0 24px}h1{font-size:42px;margin-bottom:8px}h2{font-size:24px}p{max-width:850px}section{display:grid;grid-template-columns:1fr 1fr;gap:20px}article{background:#1c3038;padding:24px;border-radius:15px}audio{width:100%;margin:12px 0}.ratings{display:grid;grid-template-columns:1fr 1fr;gap:10px}label{display:flex;justify-content:space-between;align-items:center;gap:8px;font-size:14px}select,textarea,button{font:inherit;background:#0b2028;color:#e2ecee;border:1px solid #6c8d98;border-radius:7px;padding:6px}textarea{box-sizing:border-box;width:100%;height:80px;margin-top:16px}small{display:block;color:#b2ccd0}button{padding:12px 22px;cursor:pointer;margin:24px 0}a{color:#9bd8d1}.note{padding:18px;border-left:3px solid #74bfb0;background:#172b32}@media(max-width:760px){section{grid-template-columns:1fr}h1{font-size:32px}}
    </style><h1>Composer experiment</h1><p>Original music, reference evidence, and a listening test. This first pass demonstrates a usable workflow. It does not establish professional quality or that the skill improves another model.</p><h2>Full studies</h2>'''+masters+f'''<h2>Compare excerpts</h2><p>All six passages are 19 seconds, matched by gain to {target:.1f} LUFS. A and B hide the take names. The reference titles remain visible. Reference previews have unknown positions in their original tracks, so these are exploratory comparisons; their musical functions have not been matched by audition.</p><p class="note">Rate 1 (weak) to 5 (strong). Fluency means convincing phrases, breaths and transitions. Ratings are deliberately empty. Signal measurements cannot supply these judgments.</p><section>'''+''.join(cards)+'''</section><button id="save">Download my listening ratings</button><p><a href="REPORT.md">Read the evidence report</a> · <a href="ab-key.json">Reveal A/B key</a></p><script>
    document.querySelectorAll('audio').forEach(a=>a.addEventListener('play',()=>document.querySelectorAll('audio').forEach(b=>{if(b!==a)b.pause()})));
    document.getElementById('save').onclick=()=>{const result={};document.querySelectorAll('article').forEach(a=>{const r={};a.querySelectorAll('select').forEach(s=>r[s.dataset.dimension]=s.value?Number(s.value):null);r.comment=a.querySelector('textarea').value||null;result[a.dataset.label]=r});const url=URL.createObjectURL(new Blob([JSON.stringify({date:new Date().toISOString(),ratings:result},null,2)],{type:'application/json'}));const a=document.createElement('a');a.href=url;a.download='composer-listening-ratings.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000)};
    </script></html>'''
    (root/'listening-desk.html').write_text(page)
    print('Created delivery masters, matched excerpts and listening desk; target LUFS',round(target,2))


if __name__=='__main__':main()
