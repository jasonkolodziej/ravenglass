from palette import cr
NIGHT={"background":"#0E0C1A","foreground":"#ECEAF6","cursor":"#A9DCFF",
 "black":"#0E0C1A","gray":"#1D1930","silver":"#8A85AD","white":"#ECEAF6",
 "maroon":"#B04A5A","red":"#FF7A8A","green":"#4FA37A","lime":"#7FD6A6",
 "olive":"#B08A3E","yellow":"#F2C46D","navy":"#2A2350","blue":"#9C99E6",
 "purple":"#4E48A0","fuchsia":"#D3A6FF","teal":"#3E8FB8","aqua":"#A9DCFF"}
FROST={"background":"#F4F5FB","foreground":"#17142A","cursor":"#1F6FBF",
 "black":"#F4F5FB","gray":"#E2E4F3","silver":"#6B6787","white":"#17142A",
 "maroon":"#A12A3E","red":"#C8344B","green":"#1C6B42","lime":"#22804F",
 "olive":"#7A5005","yellow":"#93600A","navy":"#D9DBF2","blue":"#4E48A0",
 "purple":"#3A3580","fuchsia":"#8A3FBF","teal":"#155A9E","aqua":"#1F6FBF"}
ROLES={ # semantic role -> palette name
 "ui.bg":"background","ui.text":"foreground","ui.muted":"silver","ui.chrome":"gray","ui.border":"gray",
 "ui.selection":"navy","ui.accent":"aqua","ui.focusBar":"aqua","ui.brand":"blue",
 "mode.normal":"aqua","mode.compose":"lime","mode.search":"yellow","mode.visual":"fuchsia",
 "mail.unread":"aqua","mail.flag":"yellow","mail.attachment":"blue","mail.quote":"silver",
 "mail.link":"aqua","mail.header.key":"blue","state.ok":"lime","state.warn":"yellow","state.error":"red",
 "wallpaper":"blue","glass.tint":"navy"}
if __name__=="__main__":
    for n,p in (("night",NIGHT),("frost",FROST)):
        print(n, {k: round(cr(p[k],p["background"]),2) for k in ("foreground","silver","aqua","lime","yellow","red","blue","fuchsia","teal")},
              "sel:", round(cr(p["foreground"],p["navy"]),2), "gray:", round(cr(p["foreground"],p["gray"]),2), "mode:",round(cr(p["black"],p["aqua"]),2))
