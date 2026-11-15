/* SPDX-License-Identifier: GPL-2.0 */
/* Validate the AOA 1.0 EP0 requests without depending on kernel types. */
static inline int aa_aoa_length(unsigned type,unsigned request,unsigned value,unsigned index,unsigned length)
{
 if(value)return -1;
 switch(request){
 case 51:return type==0xc0 && index==0 && length==2 ? 2 : -1;
 case 52:return type==0x40 && index<6 && length>0 && length<=256 ? (int)length : -1;
 case 53:return type==0x40 && index==0 && length==0 ? 0 : -1;
 default:return -1;
 }
}
