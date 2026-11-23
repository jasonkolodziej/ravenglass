from build import *
import shutil
B='/home/claude/ravenglass-brand'
# icons
for n in ('A_pane','B_etched','C_fold'): shutil.copy(f'{n}.svg',f'{B}/icon/ravenglass-icon-{n}.svg')
# watermarks: single-colour, currentColor, tight viewBox around raven (x 130-430, y 146-440)
VB='126 140 310 304'
def wm(body): return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{VB}" fill="none" stroke="currentColor" color="#000">{body}</svg>'
segs=facet_lines()
lines=''.join(f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}"/>' for a,b in segs)
etched=wm(f'<g stroke-width="1.6" stroke-opacity="0.55" stroke-linecap="round">{lines}</g><path d="{outline_d()}" stroke-width="2.6" stroke-linejoin="round"/><circle cx="{EYE[0]}" cy="{EYE[1]}" r="5" fill="currentColor" stroke="none"/>')
sil=f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{VB}" fill="currentColor" color="#000"><path fill-rule="evenodd" d="{outline_d()} M{EYE[0]+5},{EYE[1]} a5,5 0 1,0 -10,0 a5,5 0 1,0 10,0 Z"/></svg>'
open(f'{B}/watermark/ravenglass-watermark-etched.svg','w').write(etched)
open(f'{B}/watermark/ravenglass-watermark-silhouette.svg','w').write(sil)
# empty-state mock
def mock(dark):
    p=dict(bg='#0E0C1A',content='#15121F',raised='#1D1930',border='#2C2742',text='#ECEAF6',muted='#A7A2C3',faint='#7A759C',accent='#A9DCFF',wm='#A9DCFF',wmop=0.07,side='#1A1630') if dark else \
      dict(bg='#EEF0FA',content='#FFFFFF',raised='#F5F6FC',border='#DAD9EA',text='#17142A',muted='#545070',faint='#76728F',accent='#1F6FBF',wm='#4E48A0',wmop=0.07,side='#E2E5F6')
    W,H=1280,800
    rows=''
    names=[('Odin Allfather','Re: Morning dispatch','Huginn reports the northern pass is clear…',True),
           ('Muninn','Archive digest — week 46','Everything you asked me to remember, in one…',True),
           ('GitHub','[anomalyco/rune] dev: 1,007 commits','A new release of rune-kit is available…',False),
           ('Fastmail','Your invoice for November','Thanks for using Fastmail. Your receipt is…',False),
           ('Kolodziej Lab','Gnomon scheduler sync','Agenda: backfill policy, preemption…',False)]
    for i,(f,s,pv,u) in enumerate(names):
        y=96+i*78; sel=(i==0)
        rows+=f'<g><rect x="252" y="{y}" width="372" height="76" fill="{p["raised"] if sel else "none"}"/>'
        if u: rows+=f'<circle cx="268" cy="{y+22}" r="4" fill="{p["accent"]}"/>'
        rows+=f'<text x="282" y="{y+26}" font-family="Inter" font-size="14" font-weight="{700 if u else 500}" fill="{p["text"]}">{f}</text><text x="608" y="{y+26}" text-anchor="end" font-family="Inter" font-size="12" fill="{p["faint"]}">10:4{i}</text><text x="282" y="{y+46}" font-family="Inter" font-size="13" fill="{p["text"]}">{s}</text><text x="282" y="{y+64}" font-family="Inter" font-size="12.5" fill="{p["muted"]}">{pv}</text><line x1="282" y1="{y+77}" x2="624" y2="{y+77}" stroke="{p["border"]}"/></g>'
    side=''.join(f'<text x="34" y="{140+i*30}" font-family="Inter" font-size="13.5" fill="{p["text"] if i!=1 else p["accent"]}" font-weight="{600 if i==1 else 400}">{n}</text>' for i,n in enumerate(['All Inboxes','Inbox','Drafts','Sent','Archive','Flagged']))
    # watermark centred in reader (reader x 625..1280)
    cx,cy=952,420; s=1.05
    wmg=f'<g transform="translate({cx-281*s},{cy-292*s}) scale({s})" color="{p["wm"]}" opacity="{p["wmop"]/0.07*1}"><g fill="none" stroke="{p["wm"]}" stroke-opacity="{p["wmop"]}" stroke-linecap="round" stroke-linejoin="round"><g stroke-width="1.6" stroke-opacity="{p["wmop"]*0.55}">{lines}</g><path d="{outline_d()}" stroke-width="2.6"/></g><circle cx="{EYE[0]}" cy="{EYE[1]}" r="5" fill="{p["wm"]}" fill-opacity="{p["wmop"]}"/></g>'
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}">
<rect width="{W}" height="{H}" fill="{p["bg"]}"/>
<rect x="0" y="0" width="250" height="{H}" fill="{p["side"]}" opacity="0.92"/>
<rect x="0" y="0" width="{W}" height="52" fill="{p["side"]}" opacity="0.96"/><line x1="0" y1="52" x2="{W}" y2="52" stroke="{p["border"]}"/>
<circle cx="22" cy="26" r="6" fill="#ff5f57"/><circle cx="42" cy="26" r="6" fill="#febc2e"/><circle cx="62" cy="26" r="6" fill="#28c840"/>
<rect x="470" y="14" width="340" height="26" rx="8" fill="{p["content"]}" stroke="{p["border"]}"/><text x="486" y="32" font-family="Inter" font-size="13" fill="{p["faint"]}">Search mail   ⌘F</text>
<text x="34" y="100" font-family="Inter" font-size="11" font-weight="600" fill="{p["faint"]}" letter-spacing="0.6">MAILBOXES</text>{side}
<rect x="251" y="53" width="374" height="{H-53}" fill="{p["content"]}"/><line x1="250" y1="53" x2="250" y2="{H}" stroke="{p["border"]}"/>
<text x="268" y="82" font-family="Inter" font-size="15" font-weight="700" fill="{p["text"]}">Inbox</text>{rows}
<rect x="626" y="53" width="{W-626}" height="{H-53}" fill="{p["content"]}"/><line x1="625" y1="53" x2="625" y2="{H}" stroke="{p["border"]}"/>
{wmg}
<text x="952" y="{cy+188}" text-anchor="middle" font-family="Inter" font-size="15" font-weight="600" fill="{p["muted"]}">No message selected</text>
<text x="952" y="{cy+212}" text-anchor="middle" font-family="Inter" font-size="13" fill="{p["faint"]}">Press ↓ to open the next message, or ⇧⌘P for commands</text>
</svg>'''
open(f'{B}/preview/empty-state-night.svg','w').write(mock(True))
open(f'{B}/preview/empty-state-frost.svg','w').write(mock(False))
