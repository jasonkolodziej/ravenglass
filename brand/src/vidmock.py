"""Parametrized triage screen for the marketing video (same look as gridmock.triage)."""
from gridmock import T, frame, CW, LH
ROWS=[('Odin Allfather','Re: Morning dispatch','Huginn reports the northern pass is clear','10:40',True,False,True),
      ('Muninn','Archive digest — week 46','Everything you asked me to remember','10:21',True,False,False),
      ('GitHub','[anomalyco/rune] dev: new rev','rune-kit Combobox gains animated filtering','09:58',False,False,False),
      ('Fastmail','Your invoice for November','Thanks for using Fastmail. Your receipt','09:12',False,True,True),
      ('Kolodziej Lab','Gnomon scheduler sync','Agenda: backfill policy, preemption','Thu',False,True,False),
      ('Arc HPC','Job 48211 finished (COMPLETED)','Elapsed 03:12:44 on gpu-a100-07','Thu',False,False,False),
      ('Dioxus','Blitz 0.3 release notes','Incremental layout, custom elements','Wed',False,False,False),
      ('UTSA','Spring registration opens','Your enrollment window for the M.S.','Wed',False,False,True),
      ('Mom','Thanksgiving plans','Are you bringing the green chile','Tue',False,True,False),
      ('Servo','This month in Servo','Monthly releases, embedding API','Mon',False,False,False),
      ('Linear','3 issues assigned to you','RG-14 Blitz spike: Parley version','Mon',False,False,False),
      ('Cloudflare','Tunnel healthy','prefect.example.dev is serving','Sun',False,False,False),
      ('Servo','Embedding API office hours','Bring your questions about WebView','Sun',False,False,False)]
EMAIL={'Odin Allfather':'odin@asgard.example','Muninn':'muninn@ravens.example','GitHub':'notifications@github.example','Fastmail':'billing@fastmail.example','Dioxus':'news@dioxus.example'}
BODY={0:['Morning,','','Huginn reports the northern pass is clear and the','road to Uppsala is open again. Muninn has the full','archive if you need last winter\'s routes.','','Maps are attached.'],
      1:['Everything you asked me to remember this week:','','  · ferry times (winter)','  · Uppsala road closures','  · the Gnomon build checklist','','— M.'],
      2:['rune-kit Combobox gains animated filtering.','','  crates/rune-kit/src/combobox.rs  | 84 ++++---','  crates/rune-kit/README.md        |  6 +','','View it on GitHub.'],
      3:['Thanks for using Fastmail.','','Your receipt for November is attached.','','  Plan      Standard','  Period    Nov 2026','','— Fastmail billing'],
      6:['Blitz 0.3 is out','','Incremental layout, custom elements, and faster','table layout.','','  Layout   Taffy-based grid and flexbox','  Style    Stylo, the CSS engine from Firefox','','Read the notes: https://blitz.example/release/0.3']}

def triage_v(p, sel=0, removed=(), view='text', cmd=None, toast=None, W=1400, H=880):
    fg=p['foreground'];mu=p['silver'];ch=p['gray'];selc=p['navy'];acc=p['aqua']
    b=''; y0=44; LW=700
    vis=[i for i in range(len(ROWS)) if i not in removed][:12]
    unread=sum(1 for i in vis if ROWS[i][4])
    b+=T(0,0,' INBOX ',p['black'],y0,10,600,bg=acc)+T(8,0,f'iCloud · {248-len(removed)} · {unread} unread · threaded',mu,y0,10)
    spos=vis.index(sel)
    for k,i in enumerate(vis):
        f,sbj,pv,t,u,flag,att=ROWS[i]
        if i==sel: u=False
        r=2+k*2; isSel=(i==sel)
        if isSel: b+=f'<rect x="0" y="{y0+r*LH-2}" width="{LW}" height="{LH*2}" fill="{selc}"/><rect x="0" y="{y0+r*LH-2}" width="3" height="{LH*2}" fill="{acc}"/>'
        num=spos+1 if isSel else abs(k-spos)
        b+=T(0,r,f'{num:>3}',fg if isSel else mu,y0,6,600 if isSel else 400)
        b+=T(4,r,'●' if u else ' ',acc,y0,6)
        b+=T(6,r,f'{f:<16.16}',fg,y0,6,700 if u else 400)
        b+=T(23,r,f'{sbj:<36.36}',fg,y0,6,600 if u else 400)
        b+=T(73,r,'⚑' if flag else ' ',p['yellow'],y0,6)+T(75,r,'⎘' if att else ' ',p['blue'],y0,6)
        b+=T(77,r,f'{t:>5}',mu,y0,6)
        b+=T(23,r+1,f'{pv:<50.50}',mu,y0,6)
    sy=H-34-LH-6
    if toast:
        ty=sy-LH-8
        b+=f'<rect x="10" y="{ty}" width="{(len(toast)+2)*CW}" height="{LH}" fill="{p["raised"] if "raised" in p else ch}" stroke="{acc}" stroke-opacity="0.6"/>'+T(1,0,toast,fg,ty,10)
    b+=f'<rect x="0" y="{sy}" width="{LW}" height="{LH}" fill="{ch}"/>'
    if cmd is not None:
        b+=T(0,0,cmd,fg,sy,6,500)+f'<rect x="{6+len(cmd)*CW}" y="{sy+2}" width="{CW}" height="{LH-4}" fill="{p["cursor"]}"/>'
    else:
        b+=T(0,0,' NORMAL ',p['black'],sy,0,600,bg=acc)+T(9,0,'iCloud/Inbox',fg,sy,0)+T(23,0,'⎇ threaded',mu,sy,0)+T(35,0,f'● {unread}',acc,sy,0)+T(41,0,'⚑ 3',p['yellow'],sy,0)+T(66,0,f'{spos+1}/{248-len(removed)}',mu,sy,0)
    b+=T(75,0,' mail ',p['black'],sy,0,600,bg=p['blue'])
    b+=f'<line x1="{LW+2}" y1="36" x2="{LW+2}" y2="{H-34}" stroke="{ch}" stroke-width="2"/><rect x="{LW}" y="40" width="4" height="{(H-80)*0.45}" fill="{acc}"/>'
    rx=LW+16; ry=y0; RWc=int((W-40-rx-12)/CW)
    f,sbj,pv,t,u,flag,att=ROWS[sel]
    R=[('From',f'{f} <{EMAIL.get(f, f.lower().replace(" ","")+"@example.com")}>'),('To','me <jason@…>'),('Date','Fri 13 Nov 2026 '+(t if ':' in t else '09:00')),('Subject',sbj)]
    for i,(k,v) in enumerate(R): b+=T(0,i,f'{k+":":<9}',p['blue'],ry,rx,600)+T(9,i,v,fg,ry,rx,600 if k=='Subject' else 400)
    if att: b+=T(0,4,'⎘ attachment.pdf · 2.1 MB',p['blue'],ry,rx)
    if view=='html':
        b+=T(RWc-24,0,'view: ',mu,ry,rx)+T(RWc-18,0,'html',acc,ry,rx,600)+T(RWc-13,0,'│ t text',mu,ry,rx)
    else:
        b+=T(RWc-24,0,'view: ',mu,ry,rx)+T(RWc-18,0,'text',acc,ry,rx,600)+T(RWc-13,0,'│ t html',mu,ry,rx)
    b+=f'<line x1="{rx}" y1="{ry+5.6*LH}" x2="{W-52}" y2="{ry+5.6*LH}" stroke="{ch}"/>'
    if view=='html':
        # a rendered HTML newsletter on its paper card (Blitz output in the real app)
        cx,cy,cw,chh=rx,ry+6.6*LH,W-52-rx,700-ry
        b+=f'<rect x="{cx}" y="{cy}" width="{cw}" height="{chh}" rx="6" fill="#EEF0F4"/>'
        ix,iy,iw=cx+40,cy+24,cw-80
        b+=f'<rect x="{ix}" y="{iy}" width="{iw}" height="{chh-48}" rx="8" fill="#FFFFFF"/>'
        b+=f'<rect x="{ix}" y="{iy}" width="{iw}" height="120" rx="8" fill="#5B4FD6"/><rect x="{ix}" y="{iy+112}" width="{iw}" height="8" fill="#5B4FD6"/>'
        b+=f'<g fill="#FFFFFF" opacity="0.25"><circle cx="{ix+iw-70}" cy="{iy+60}" r="38"/><circle cx="{ix+iw-140}" cy="{iy+40}" r="18"/></g>'
        b+=f'<text x="{ix+24}" y="{iy+70}" font-family="Helvetica, Arial, sans-serif" font-size="30" font-weight="700" fill="#FFFFFF">Blitz 0.3 is out</text>'
        b+=f'<text x="{ix+24}" y="{iy+160}" font-family="Helvetica, Arial, sans-serif" font-size="16" fill="#4A4860">Incremental layout, custom elements, and faster</text><text x="{ix+24}" y="{iy+182}" font-family="Helvetica, Arial, sans-serif" font-size="16" fill="#4A4860">table layout.</text>'
        b+=f'<rect x="{ix+24}" y="{iy+204}" width="170" height="40" rx="6" fill="#5B4FD6"/><text x="{ix+109}" y="{iy+230}" text-anchor="middle" font-family="Helvetica, Arial, sans-serif" font-size="15" font-weight="700" fill="#FFFFFF">Read the notes</text>'
        b+=f'<line x1="{ix+24}" y1="{iy+268}" x2="{ix+iw-24}" y2="{iy+268}" stroke="#E4E3EE"/>'
        for j,(h,d) in enumerate((('Layout','Taffy grid and flexbox.'),('Style','Stylo, from Firefox.'))):
            xx=ix+24+j*(iw-48)/2
            b+=f'<text x="{xx}" y="{iy+300}" font-family="Helvetica, Arial, sans-serif" font-size="16" font-weight="700" fill="#1B1A2E">{h}</text><text x="{xx}" y="{iy+322}" font-family="Helvetica, Arial, sans-serif" font-size="14" fill="#4A4860">{d}</text>'
        b+=f'<text x="{ix+iw/2}" y="{iy+chh-70}" text-anchor="middle" font-family="Helvetica, Arial, sans-serif" font-size="12" fill="#8A88A0">Unsubscribe · remote images blocked</text>'
    else:
        for i,l in enumerate(BODY.get(sel,[pv+'.','','— '+f])): b+=T(0,7+i,l,fg,ry,rx)
    tabs=[('Inbox — iCloud',True),(sbj[:22],True),('Draft: Gnomon sync',False)]
    return frame(p,W,H,tabs,b)
