#!/usr/bin/env python3
# Usage: gen_codepages.py <go-activesync wbxml/codepages.go> <Z-Push lib/wbxml/wbxmldefs.php> <out codepages.rs>
import re,sys
go=open(sys.argv[1]).read(); zp=open(sys.argv[2]).read()
gp={}
for m in re.finditer(r'Number:\s*Page(\w+),.*?Tags: map\[byte\]string\{(.*?)\n\t\t\}',go,re.S):
    gp[m.group(1)]=[(int(a,16),b) for a,b in re.findall(r'(0x[0-9A-Fa-f]+):\s*"([^"]+)"',m.group(2))]
names=["AirSync","Contacts","Email","AirNotify","Calendar","Move","GetItemEstimate","FolderHierarchy","MeetingResponse","Tasks","ResolveRecipients","ValidateCert","Contacts2","Ping","Provision","Search","GAL","AirSyncBase","Settings","DocumentLibrary","ItemOperations","ComposeMail","Email2","Notes","RightsManagement","Find"]
codes=zp[zp.index('"codes"'):zp.index('"namespaces"')]
zpages={};cur=None
for line in codes.splitlines():
    m=re.match(r'\s*(0x[0-9A-Fa-f]+|\d+)\s*=>\s*array\s*\(',line)
    if m: cur=int(m.group(1),0); zpages[cur]={}; continue
    m=re.match(r'\s*(0x[0-9A-Fa-f]+)\s*=>\s*"([^"]+)"',line)
    if m and cur is not None: zpages[cur][int(m.group(1),16)]=m.group(2)
out=["// Code page tables for EAS WBXML (MS-ASWBXML section 2.1.2.1).","//","// Official tag names come from go-activesync (MIT, (c) 2026 Henry Stern);","// tokens missing there (mostly EAS 16.x additions) come from Z-Push","// lib/wbxml/wbxmldefs.php (AGPL-3.0). Regenerate with tools/gen_codepages.py;","// do not edit by hand.","","/// Namespace name per code page.","pub const NAMESPACES: [&str; 26] = ["]
out+=[f'    "{n}",' for n in names]+["];","","/// (token, tag name) pairs per code page, sorted by token.","pub const PAGES: [&[(u8, &str)]; 26] = ["]
added=0
for p,n in enumerate(names):
    d=dict(gp.get(n,[]))
    for t,nm in zpages.get(p,{}).items():
        if t not in d: d[t]=nm; added+=1
    out.append(f"    // 0x{p:02X} {n}")
    out.append("    &["+", ".join(f'(0x{t:02X}, "{nm}")' for t,nm in sorted(d.items()))+"],")
out.append("];")
open(sys.argv[3],"w").write("\n".join(out)+"\n")
print("go pages",len(gp),"tags",sum(map(len,gp.values())),"added from zpush",added,file=sys.stderr)
