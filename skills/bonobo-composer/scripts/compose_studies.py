#!/usr/bin/env python3
"""Create original, editable demonstration studies; Python standard library only."""
import argparse
import copy
import json
import math
import random
import struct
import wave
from pathlib import Path

SR = 44100


def write_wav(path, seconds, fn):
    count = int(SR * seconds)
    samples = [max(-0.97, min(0.97, fn(i / SR))) for i in range(count)]
    with wave.open(str(path), 'wb') as w:
        w.setparams((1, 2, SR, 0, 'NONE', 'not compressed'))
        w.writeframes(struct.pack('<'+'h'*count, *(round(v*32767) for v in samples)))


def assets(directory):
    directory.mkdir(parents=True, exist_ok=True)
    rng = random.Random(181098)
    write_wav(directory/'kick.wav', .72, lambda t: .82*math.sin(2*math.pi*(49*t+1.6*(1-math.exp(-t*46))))*math.exp(-t*10)*min(1,t*1800))
    for label, decay, level in [('soft', 23, .40), ('hard', 17, .70)]:
        write_wav(directory/f'snare-{label}.wav', .4, lambda t: level*(rng.uniform(-1,1)*.70+math.sin(2*math.pi*186*t)*.30)*math.exp(-t*decay)*min(1,t*2400))
    for label, decay in [('hat', 67), ('open', 18), ('brush', 34)]:
        previous = [0.]
        def noise(t):
            x = rng.uniform(-1,1)
            v = x-previous[0]
            previous[0] = x
            return .35*v*math.exp(-t*decay)*min(1,t*4000)
        write_wav(directory/f'{label}.wav', .36, noise)
    write_wav(directory/'wood.wav', .18, lambda t: .50*(math.sin(2*math.pi*960*t)+.5*math.sin(2*math.pi*1471*t))*math.exp(-t*58)*min(1,t*3000))
    for pitch in [68,70,72,75,77,79,80,82,84,87]:
        f = 440*2**((pitch-69)/12)
        def pluck(t):
            return .42*(math.sin(2*math.pi*f*t)*math.exp(-2.8*t)+.32*math.sin(2*math.pi*f*2.76*t)*math.exp(-7*t)+.11*math.sin(2*math.pi*f*5.4*t)*math.exp(-16*t))*min(1,t*900)*min(1,(1.5-t)*40)
        write_wav(directory/f'tine-{pitch}.wav',1.5,pluck)
    # Own synthetic tones only; none of the reference recordings are sampled.
    return directory


def project(name, bpm, meter=(4,4)):
    return dict(version='0.1', project=dict(name=name,bpm=bpm,time_signature=list(meter),key='F',scale='minor',swing=0.,humanize=0.,timing=0.,metronome=False),tracks=[],patterns=[],arrangement=[],audio_clips=[],
                returns=dict(reverb=dict(gain=.64,room_size=.67,damping=.60,width=.9,predelay_ms=18,low_cut_hz=220,high_cut_hz=6500),delay=dict(gain=.55,time_ms=459,feedback=.25,sync='eighth_dotted',mode='ping_pong',low_cut_hz=320,high_cut_hz=4200,width=.8,modulation_ms=1.7,modulation_hz=.18)),loop_region=dict(enabled=False,start_bar=8,end_bar=16))


def track(p, tid, name, volume, preset=0, pan=0., pads=None, **fx):
    inst = dict(type='soundfont',preset=preset,bank=0)
    if pads:
        inst = dict(type='sampler',pads=[dict(pitch=k,path='assets/'+v,gain=1.) for k,v in pads])
    t = dict(id=tid,name=name,type='drum' if pads else 'melodic',instrument=inst,volume=volume,pan=pan,muted=False,patterns=[],effects=fx,modulators=[])
    p['tracks'].append(t)
    return t


def note(pitch, tick, duration, vel):
    return dict(pitch=pitch,tick=int(tick),duration=int(duration),vel=int(vel))


def pattern(p, tid, pid, notes, bars=8, ticks=480):
    p['patterns'].append(dict(type='piano_roll',id=pid,name=pid,bars=bars,ticks_per_bar=ticks,notes=notes))
    next(t for t in p['tracks'] if t['id']==tid)['patterns'].append(pid)


def clip(p, tid, pid, bar, length):
    p['arrangement'].append(dict(track=tid,pattern=pid,bar=bar,len_bars=length))


def compose_main():
    p = project('Estuary — developed study',98.)
    track(p,'kick','Low felt',.24,pads=[(36,'kick.wav')],eq_high_gain=-7)
    t=track(p,'snare','Paper and skin',.55,pan=-.035,pads=[(38,'snare-hard.wav')],reverb=.10,eq_low_gain=-6,eq_high_gain=-2)
    t['instrument']['pads'][0]['variants']=[dict(path='assets/snare-soft.wav',min_velocity=1,max_velocity=63),dict(path='assets/snare-hard.wav',min_velocity=64,max_velocity=127)]
    track(p,'hat','Quiet copper',.30,pan=.20,pads=[(42,'hat.wav'),(46,'open.wav')],eq_low_gain=-12)
    track(p,'wood','Rotating wood',.23,pan=-.32,pads=[(37,'wood.wav'),(70,'brush.wav')],reverb=.13,send_delay=.15,eq_low_gain=-9)
    track(p,'bass','Current',1.70,preset=38,cutoff=.48,eq_high_gain=-8)
    track(p,'keys','Close glass',.80,preset=4,pan=-.16,reverb=.16,send_delay=.10,eq_low_gain=-5)
    track(p,'tine','Tine collage',.65,pan=.26,pads=[(i,f'tine-{i}.wav') for i in [68,70,72,75,77,79,80,82,84,87]],reverb=.23,send_delay=.22,eq_low_gain=-9)
    track(p,'guitar','Nylon replies',.60,preset=24,pan=-.29,reverb=.19,send_delay=.12,eq_low_gain=-8)
    track(p,'strings','Long horizon',.42,preset=48,pan=-.10,reverb=.28,eq_low_gain=-9,cutoff=.64)
    track(p,'flute','Breathing answer',.65,preset=73,pan=.10,reverb=.22,send_delay=.17,eq_low_gain=-8)
    roots=[41,41,37,37,44,44,39,39]
    chords=[[56,60,63,67]]*2+[[56,60,63,65]]*2+[[55,58,60,63]]*2+[[55,58,60,65]]*2
    for variant in ['seed','answer','return','thin']:
        ns={tid:[] for tid in ['kick','snare','hat','wood','bass','keys','tine','guitar','strings','flute']}
        for b in range(8):
            q=b*480
            sparse=variant=='thin'
            kick=[(0,105),(210,75),(300,93)] if b%2==0 else [(0,100),(180,70),(330,87),(450,59)]
            if sparse:kick=[(0,81)] if b%2==0 else []
            if variant=='return' and b in [3,7]:kick.append((390,66))
            ns['kick'] += [note(36,q+t,50,v) for t,v in kick]
            if not sparse:
                ns['snare'] += [note(38,q+122,32,95),note(38,q+363,32,102)]
                if b%2:ns['snare'].append(note(38,q+333,20,35))
                if variant in ['answer','return'] and b==7:ns['snare'] += [note(38,q+423,20,43),note(38,q+453,20,60)]
            for i in range(8):
                if sparse and i%2:continue
                if i==7 and b%4==3:continue
                ns['hat'].append(note(42,q+i*60+(3 if i%2 else 0),16,[62,35,49,30,58,39,45,32][i]+b%3))
            if b%4==2 and not sparse:ns['hat'].append(note(46,q+435,22,40))
            root=roots[b]; ch=chords[b]
            ns['bass'] += [note(root,q+1,167,87),note(root,q+270,126,75)]
            if b%2 and not sparse:ns['bass'].append(note(root+7,q+426,44,65))
            onset=0 if b%2==0 else 63
            ns['keys'] += [note(k,q+onset+j*2,322 if b%2==0 else 279,58+j*2) for j,k in enumerate(ch)]
            if variant=='return' and b%2==1:ns['keys'] += [note(k,q+401+j*2,60,37+j*2) for j,k in enumerate(ch[1:])]
            ns['guitar'] += [note(ch[1]+12,q+184,77,48),note(ch[2]+12,q+394,66,42)]
            if b%2==0:ns['strings'] += [note(k+12,q+12+j*3,874,43) for j,k in enumerate(ch[::2])]
        # One identifiable contour; a different second ending and register on return.
        melody=[(0,72,62,80),(210,75,58,65),(300,79,90,74),(600,77,68,71),(780,75,90,62),(960,72,145,73),(1230,70,63,62),(1380,68,95,68),
                (1920,72,60,78),(2130,75,50,63),(2220,79,92,72),(2520,80,75,76),(2730,77,105,65),(3000,75,100,70),(3330,72,150,62)]
        if variant=='answer':melody=melody[:8]+[(1920,72,60,77),(2130,75,55,64),(2250,82,80,72),(2490,80,95,70),(2790,77,120,65),(3180,75,160,61)]
        if variant=='return':melody=melody[:8]+[(1920,84,60,75),(2130,82,65,67),(2310,79,110,71),(2610,77,90,65),(2850,75,120,63),(3240,72,220,57)]
        if variant=='thin':melody=[(0,72,95,58),(660,75,100,50),(1380,68,130,52),(2220,79,160,53),(3180,75,180,47)]
        ns['tine']=[note(k,t,d,v) for t,k,d,v in melody]
        # 3/8 secondary cycle phased across 4/4; lower velocity and sparse placement.
        for t in range(90,3840,180):
            if (t//180)%3!=1:ns['wood'].append(note(37 if (t//180)%2 else 70,t+2,22,35+(t//180)%4*5))
        answer=[(420,79,150,61),(720,77,160,58),(1170,75,250,64),(1650,72,190,54),(2370,75,150,61),(2700,77,210,64),(3150,79,160,60),(3450,77,190,53)]
        if variant=='return':answer=[(420,79,160,60),(780,82,130,63),(1140,80,260,65),(1650,79,150,57),(2370,77,190,59),(2760,75,160,57),(3180,72,270,53)]
        ns['flute']=[note(k,t,d,v) for t,k,d,v in answer]
        for tid,notes in ns.items():pattern(p,tid,tid+'_'+variant,notes)
    sections=[(0,8,'thin',['keys','tine','guitar']),
              (8,8,'seed',['kick','snare','hat','bass','keys','tine']),
              (16,8,'answer',['kick','snare','hat','wood','bass','keys','tine','guitar','flute']),
              (24,8,'thin',['keys','guitar','strings','tine']),
              (32,8,'return',['kick','snare','hat','wood','bass','keys','tine','strings']),
              (40,8,'return',['kick','snare','hat','wood','bass','keys','tine','guitar','flute']),
              (48,8,'thin',['keys','guitar','strings','tine'])]
    for start,bars,var,tids in sections:
        for tid in tids:clip(p,tid,tid+'_'+var,start,bars)
    # Last phrase thins within the outro instead of merely lowering delivery gain.
    for tid in ['keys','guitar','strings','tine']:
        source=next(x for x in p['patterns'] if x['id']==tid+'_thin')
        notes=[n for n in source['notes'] if n['tick']<6*480]
        if tid=='keys':notes += [note(k,6*480+j*2,780,44+j*2) for j,k in enumerate([56,60,63,67])]
        if tid=='tine':notes += [note(72,6*480,220,49)]
        pattern(p,tid,tid+'_ending',notes)
        next(x for x in p['arrangement'] if x['track']==tid and x['bar']==48)['pattern']=tid+'_ending'
    control=copy.deepcopy(p)
    control['project']['name']='Estuary — loop arrangement ablation'
    control['arrangement']=[]
    for tid in ['kick','snare','hat','wood','bass','keys','tine','guitar']:
        clip(control,tid,tid+'_seed',0,56)
    return p,control


def compose_compound():
    p=project('Three Lanterns — compound-meter study',90.,(6,8))
    for tid,name,vol,preset,pan in [('nylon','Nylon in six',.58,24,-.22),('upright','Two pulses',.66,32,0),('clarinet','Long breath',.40,71,.12),('strings','Distant tide',.20,48,-.10)]:
        track(p,tid,name,vol,preset=preset,pan=pan,reverb=.18 if tid!='strings' else .3,eq_low_gain=-7 if tid in ['strings','clarinet'] else 0)
    chords=[[53,56,60,67],[53,56,60,63],[51,55,58,65],[53,56,60,63]]
    roots=[41,37,39,41]
    for var in ['a','b','end']:
        ns={t:[] for t in ['nylon','upright','clarinet','strings']}
        for b in range(8):
            q=b*360; ch=chords[b//2]; root=roots[b//2]
            order=[0,2,1,3,1,2]
            for i,j in enumerate(order):ns['nylon'].append(note(ch[j]+12,q+i*60+(1 if i%3 else 0),66,[77,51,58,69,49,55][i]))
            ns['upright'] += [note(root,q,151,83),note(root+7 if b%2 else root,q+181,148,66)]
            if b%2==0:ns['strings'] += [note(k+12,q+3+j*2,658,40) for j,k in enumerate(ch[::2])]
        line=[(180,72,160,62),(420,75,130,67),(720,79,300,70),(1140,77,170,62),(1440,75,290,65),(1860,70,170,57),(2160,72,320,63)]
        if var=='b':line=[(180,75,160,64),(420,77,130,66),(720,80,300,70),(1140,79,170,64),(1440,77,280,65),(1860,75,170,58),(2160,72,360,60)]
        ns['clarinet']=[note(k,t,d,v) for t,k,d,v in line]
        if var=='end':
            for tid in ns:ns[tid]=[n for n in ns[tid] if n['tick']<2160]
            ns['nylon'] += [note(k+12,2160+j*7,625,51+j) for j,k in enumerate(chords[0])]
            ns['upright'].append(note(41,2160,620,66))
            ns['clarinet'].append(note(72,2190,510,53))
        for tid,notes in ns.items():pattern(p,tid,tid+'_'+var,notes,ticks=360)
    for start,var in [(0,'a'),(8,'b'),(16,'end')]:
        for tid in ['nylon','upright','clarinet','strings']:
            if start==0 and tid=='strings':continue
            clip(p,tid,tid+'_'+var,start,8)
    # The current daw-ai renderer rejects non-4/4 projects. Pack an uninterrupted
    # 24x360-tick musical performance into 18x480-tick engine container bars.
    source=copy.deepcopy(p)
    p['project']['time_signature']=[4,4]
    p['project']['name']+=' [6/8 phrasing in 4/4 container]'
    p['patterns']=[];p['arrangement']=[]
    for t in p['tracks']:
        tid=t['id'];t['patterns']=[];notes=[]
        for start,var in [(0,'a'),(8,'b'),(16,'end')]:
            if start==0 and tid=='strings':continue
            pr=next(x for x in source['patterns'] if x['id']==tid+'_'+var)
            notes += [dict(n,tick=n['tick']+start*360) for n in pr['notes']]
        pattern(p,tid,tid+'_packed_six',notes,bars=18,ticks=480)
        clip(p,tid,tid+'_packed_six',0,18)
    return p


def write_midi(p,path,meter=(4,4)):
    """Standard MIDI for import into an existing DAW; sampler timbres use GM."""
    def vlq(n):
        data=[n&127];n>>=7
        while n:data.insert(0,128|(n&127));n>>=7
        return bytes(data)
    def chunk(events,end_tick=None):
        last=0;data=b''
        for tick,event in sorted(events,key=lambda x:x[0]):
            data+=vlq(tick-last)+event;last=tick
        data+=vlq(max(0,(end_tick or last)-last))+b'\xff\x2f\x00'
        return b'MTrk'+struct.pack('>I',len(data))+data
    tempo=round(60_000_000/p['project']['bpm'])
    data=chunk([(0,b'\xff\x51\x03'+tempo.to_bytes(3,'big')),(0,b'\xff\x58\x04'+bytes([meter[0],int(math.log2(meter[1])),36 if meter==(6,8) else 24,8]))])
    for idx,t in enumerate(p['tracks']):
        drum=t['id'] in ['kick','snare','hat','wood']
        ch=9 if drum else (idx if idx<9 else idx+1)
        name=t['name'].encode();program=t['instrument'].get('preset',108)
        events=[(0,b'\xff\x03'+vlq(len(name))+name),(0,bytes([0xc0|ch,program]))]
        for slot in p['arrangement']:
            if slot['track']!=t['id']:continue
            pr=next(x for x in p['patterns'] if x['id']==slot['pattern'])
            length=slot.get('len_bars',pr['bars'])*480
            natural=pr['bars']*pr['ticks_per_bar'];start=slot['bar']*480
            for repeat in range(0,length,natural):
                for n in pr['notes']:
                    tick=repeat+n['tick']
                    if tick>=length:continue
                    events += [(start+tick,bytes([0x90|ch,n['pitch'],n['vel']])),(start+min(tick+n['duration'],length),bytes([0x80|ch,n['pitch'],0]))]
        end_tick=max((s['bar']*480+s.get('len_bars',next(x for x in p['patterns'] if x['id']==s['pattern'])['bars'])*480 for s in p['arrangement']),default=0)
        data+=chunk(events,end_tick)
    path.write_bytes(b'MThd'+struct.pack('>IHHH',6,1,len(p['tracks'])+1,120)+data)


def main():
    a=argparse.ArgumentParser(description=__doc__)
    a.add_argument('--output',required=True,type=Path)
    args=a.parse_args(); out=args.output
    out.mkdir(parents=True,exist_ok=True)
    if any((out/(stem+'.daw.json')).exists() for stem in ['estuary','estuary-loop','three-lanterns']):
        raise FileExistsError('Use a new output folder to preserve earlier takes')
    assets(out/'assets')
    main,control=compose_main(); compound=compose_compound()
    for stem,p in [('estuary',main),('estuary-loop',control),('three-lanterns',compound)]:
        path=out/(stem+'.daw.json')
        if path.exists():raise FileExistsError(f'Refusing to overwrite {path}; use a new output folder')
        path.write_text(json.dumps(p,indent=2)+'\n')
        write_midi(p,out/(stem+'.mid'),(6,8) if stem=='three-lanterns' else (4,4))
        print(path, len(p['tracks']),'tracks',len(p['patterns']),'patterns')
    (out/'composition-notes.json').write_text(json.dumps(dict(seed=181098,main=dict(title='Estuary',bpm=98,bars=56,seconds=56*240/98,tonality='F minor',sections=['0–8 intro','8–16 seed','16–24 answer','24–32 break','32–40 return','40–48 second answer','48–56 ending']),compound=dict(title='Three Lanterns',bpm_quarter=90,musical_meter=[6,8],musical_ticks_per_bar=360,musical_bars=24,engine_meter=[4,4],engine_ticks_per_bar=480,engine_bars=18,seconds=48,midi_meter=[6,8]),control='Explicit arrangement ablation, not a no-skill model trial; same seed notes and palette, repeated throughout.',samples='Original deterministic synthesis; SoundFont accompaniment uses installed GeneralUser GS.'),indent=2)+'\n')


if __name__=='__main__':main()
