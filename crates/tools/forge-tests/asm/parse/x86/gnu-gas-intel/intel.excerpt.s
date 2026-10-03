.text
.intel_syntax noprefix
foo:
 add    byte ptr 0x90909090[eax], dl
 add    dword ptr 0x90909090[eax], edx
 add    dl, byte ptr 0x90909090[eax]
 add    edx, dword ptr 0x90909090[eax]
 add    al, 0x90
 add    eax, 0x90909090
 push   es
 pop    es
 or     [eax+0x90909090], dl
 or     [eax+0x90909090], edx
 or     dl, [eax+0x90909090]
 or     edx, [eax+0x90909090]
 or     al, 0x90
 or     eax, 0x90909090
 push   cs
 adc    byte ptr [eax+0x90909090], dl
 adc    dword ptr [eax+0x90909090], edx
 adc    dl, byte ptr [eax+0x90909090]
 adc    edx, dword ptr [eax+0x90909090]
 adc    al, 0x90
 adc    eax, 0x90909090
 push   ss
 pop    ss
 sbb    0x90909090[eax], dl
 sbb    0x90909090[eax], edx
 sbb    dl, 0x90909090[eax]
 sbb    edx, 0x90909090[eax]
 sbb    al, 0x90
 sbb    eax, 0x90909090
 push   ds
 pop    ds
 and    0x90909090[eax], dl
 and    0x90909090[eax], edx
 and    dl, 0x90909090[eax]
 and    edx, 0x90909090[eax]
 and    al, 0x90
 and    eax, 0x90909090
 daa
 sub    0x90909090[eax], dl
 sub    0x90909090[eax], edx
 sub    dl, 0x90909090[eax]
 sub    edx, 0x90909090[eax]
 sub    al, 0x90
 sub    eax, 0x90909090
 das
 xor    0x90909090[eax], dl
 xor    0x90909090[eax], edx
 xor    dl, 0x90909090[eax]
 xor    edx, 0x90909090[eax]
 xor    al, 0x90
 xor    eax, 0x90909090
 aaa
 cmp    0x90909090[eax], dl
 cmp    0x90909090[eax], edx
 cmp    dl, 0x90909090[eax]
 cmp    edx, 0x90909090[eax]
 cmp    al, 0x90
 cmp    eax, 0x90909090
 aas
 inc    eax
 inc    ecx
