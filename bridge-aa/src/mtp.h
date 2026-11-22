/* SPDX-License-Identifier: GPL-3.0-or-later
 * Minimal read-only MTP discovery responder. No storage or files are exposed.
 * DeviceInfo layout: PTP/MTP; compatibility reference: aa-proxy-rs src/mtp.
 */
#ifndef CARFLUXOID_MTP_H
#define CARFLUXOID_MTP_H
#include <stdint.h>
#include <stddef.h>
#include <string.h>
static uint16_t mtp_u16(const unsigned char *p){return (uint16_t)p[0]|(uint16_t)p[1]<<8;}
static uint32_t mtp_u32(const unsigned char *p){return (uint32_t)p[0]|(uint32_t)p[1]<<8|(uint32_t)p[2]<<16|(uint32_t)p[3]<<24;}
static void mtp_w16(unsigned char *p,uint16_t v){p[0]=v;p[1]=v>>8;}
static void mtp_w32(unsigned char *p,uint32_t v){p[0]=v;p[1]=v>>8;p[2]=v>>16;p[3]=v>>24;}
static void mtp_string(unsigned char *p,size_t *at,const char *s){
 size_t n=strlen(s);p[(*at)++]=(unsigned char)(n+1);
 for(size_t i=0;i<=n;i++){mtp_w16(p+*at,(unsigned char)s[i]);*at+=2;}
}
struct mtp_reply {unsigned char data[512],response[16];size_t data_size,response_size;};
static int mtp_command(const unsigned char *cmd,size_t size,uint32_t *session,struct mtp_reply *r){
 if(size<12 || size>32 || size%4 || mtp_u32(cmd)!=size || mtp_u16(cmd+4)!=1)return -1;
 memset(r,0,sizeof *r);uint16_t op=mtp_u16(cmd+6),status=0x2001;uint32_t tx=mtp_u32(cmd+8);size_t at=12;
 if(op==0x1001){
  mtp_w16(r->data+at,100);at+=2;mtp_w32(r->data+at,6);at+=4;
  mtp_w16(r->data+at,100);at+=2;mtp_string(r->data,&at,"microsoft.com: 1.0; android.com: 1.0;");
  mtp_w16(r->data+at,0);at+=2;
  const uint16_t ops[]={0x1001,0x1002,0x1003,0x1004,0x1006,0x1007};
  mtp_w32(r->data+at,6);at+=4;for(unsigned i=0;i<6;i++){mtp_w16(r->data+at,ops[i]);at+=2;}
  for(unsigned i=0;i<4;i++){mtp_w32(r->data+at,0);at+=4;}
  mtp_string(r->data,&at,"CarFluxoid");mtp_string(r->data,&at,"CarFluxoid");
  mtp_string(r->data,&at,"1.0");mtp_string(r->data,&at,"CARFLUXOID_USB");
 }else if(op==0x1002){
  if(size!=16 || !mtp_u32(cmd+12))status=0x201d;
  else if(*session)status=0x201e;else *session=mtp_u32(cmd+12);
 }else if(op==0x1003){if(!*session)status=0x2003;else *session=0;
 }else if(op==0x1004 || op==0x1006 || op==0x1007){
  if(!*session)status=0x2003;
  else if(op==0x1006){mtp_w32(r->response+12,0);r->response_size=16;}
  else {mtp_w32(r->data+at,0);at+=4;}
 }else status=0x2005;
 if(at>12 && status==0x2001){r->data_size=at;mtp_w32(r->data,(uint32_t)at);mtp_w16(r->data+4,2);mtp_w16(r->data+6,op);mtp_w32(r->data+8,tx);}
 if(!r->response_size)r->response_size=12;
 mtp_w32(r->response,(uint32_t)r->response_size);mtp_w16(r->response+4,3);mtp_w16(r->response+6,status);mtp_w32(r->response+8,tx);return 0;
}
#endif
