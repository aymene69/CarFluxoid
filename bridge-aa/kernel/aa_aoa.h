/* SPDX-License-Identifier: GPL-2.0 */
/* Validate the AOA 1.0 EP0 requests without depending on kernel types. */
static inline int aa_aoa_length(unsigned type,unsigned request,unsigned value,unsigned index,unsigned length)
{
 if(value)return -1;
 switch(request){
 case 51:return type==0xc0 && index==0 && length==2 ? 2 : -1;
 case 52:return type==0x40 && index<6 && length>0 && length<=256 ? (int)length : -1;
 case 53:return type==0x40 && index==0 && length==0 ? 0 : -1;
 case 58:return type==0x40 && index==0 && length==0 ? 0 : -1; /* Disable deprecated USB audio only. */
 default:return -1;
 }
}

/* Android f_accessory iInterface. Uconnect VP2 USBAAPTRANSPORT.DLL claims an
 * accessory interface only if this LANGID 0x0409 string matches exactly. */
#define AA_ACCESSORY_INTERFACE_NAME "Android Accessory Interface"

/* Transport compatibility profile; callers reject values other than 1 and 2. */
static inline int aa_aoa_protocol_reply(unsigned version,unsigned char *reply)
{
 if(version!=1 && version!=2)return -1;
 reply[0]=(unsigned char)version;reply[1]=0;return 2;
}
