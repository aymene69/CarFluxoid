// SPDX-License-Identifier: GPL-2.0
#include <linux/module.h>
#include <linux/usb/composite.h>
#include <linux/miscdevice.h>
#include <linux/kfifo.h>
#include <linux/wait.h>
#include <linux/completion.h>
#include <linux/uaccess.h>
#include <linux/mutex.h>
#include <linux/workqueue.h>
#include <linux/delay.h>
#include "aa_aoa.h"
#define DEPTH 8
#define CHUNK 4096
static struct usb_ep *epin,*epout;
static struct usb_function function;
static struct usb_request *rx[DEPTH];
static DECLARE_KFIFO(fifo,u8,131072);
static DEFINE_SPINLOCK(rxlock);
static DECLARE_WAIT_QUEUE_HEAD(rxwait);
static DEFINE_MUTEX(readlock);
static DEFINE_MUTEX(writelock);
static bool online,overflow,disconnected;
static unsigned long usb_generation;
static int aa_open(struct inode *inode,struct file *file){file->private_data=(void *)READ_ONCE(usb_generation);return 0;}
module_param_named(usb_online,online,bool,0444);
/* Keep direct accessory mode available for A/B comparison with the validated driver. */
static bool pre_accessory;
module_param(pre_accessory,bool,0444);
static bool accessory=true;
module_param_named(accessory_active,accessory,bool,0444);
static bool legacy_mtp;
module_param(legacy_mtp,bool,0444);
static unsigned int mtp_fallback_ms;
module_param(mtp_fallback_ms,uint,0444);
static void mtp_timeout(struct work_struct *work);
static DECLARE_DELAYED_WORK(mtp_timeout_work,mtp_timeout);
static struct usb_ep *epnotify;
static bool notification_online;
static unsigned int aoa_requests;
module_param(aoa_requests,uint,0444);
/* Completion counters distinguish a received SETUP from a delivered EP0 reply. */
static unsigned int aoa_completed,aoa_errors,aoa_unsupported;
static unsigned int vendor_probes;
static int aoa_last_status;
module_param(aoa_completed,uint,0444);
module_param(aoa_errors,uint,0444);
module_param(aoa_unsupported,uint,0444);
module_param(aoa_last_status,int,0444);
/* Explicit AA compatibility profile, as used by aa-proxy-rs. HID requests
 * remain unsupported: this is not a general Android HID/input implementation. */
static unsigned int aoa_protocol=1;
static int protocol_set(const char *value,const struct kernel_param *kp){
 unsigned int version;int ret=kstrtouint(value,0,&version);
 if(ret)return ret;if(version!=1 && version!=2)return -EINVAL;
 WRITE_ONCE(*(unsigned int *)kp->arg,version);return 0;
}
static const struct kernel_param_ops protocol_ops={.set=protocol_set,.get=param_get_uint};
module_param_cb(aoa_protocol,&protocol_ops,&aoa_protocol,0644);
static struct usb_composite_dev *aoa_cdev;
static void aoa_switch(struct work_struct *work);
static DECLARE_DELAYED_WORK(aoa_work,aoa_switch);
static unsigned long rx_bytes,tx_bytes;
module_param(rx_bytes,ulong,0444);module_param(tx_bytes,ulong,0444);

static void rx_done(struct usb_ep *ep,struct usb_request *r){
 unsigned long flags;unsigned copied;
 if(r->status)return;
 if(r->actual>CHUNK){WRITE_ONCE(overflow,true);pr_err("aa_bulk: invalid RX length %u\n",r->actual);wake_up_interruptible(&rxwait);return;}
 spin_lock_irqsave(&rxlock,flags);
 copied=kfifo_in(&fifo,r->buf,r->actual);rx_bytes+=copied;
 if(copied!=r->actual)overflow=true;
 spin_unlock_irqrestore(&rxlock,flags);
 if(READ_ONCE(online)&&!READ_ONCE(overflow)){
  int ret=usb_ep_queue(ep,r,GFP_ATOMIC);if(ret){WRITE_ONCE(overflow,true);pr_err("aa_bulk: RX requeue %d\n",ret);}
 }
 wake_up_interruptible(&rxwait);
}
static void tx_done(struct usb_ep *ep,struct usb_request *r){complete(r->context);}
static ssize_t aa_read(struct file *f,char __user *buf,size_t n,loff_t *off){
 unsigned char *tmp;unsigned copied;unsigned long flags;int ret;
 if(!n)return 0;
 ret=mutex_lock_interruptible(&readlock);if(ret)return ret;
 ret=wait_event_interruptible(rxwait,(unsigned long)f->private_data!=READ_ONCE(usb_generation)||!kfifo_is_empty(&fifo)||READ_ONCE(disconnected)||READ_ONCE(overflow));
 if(ret)goto unlock;
 if((unsigned long)f->private_data!=READ_ONCE(usb_generation)){ret=-ENODEV;goto unlock;}
 if(READ_ONCE(overflow)){ret=-EOVERFLOW;goto unlock;}
 if(kfifo_is_empty(&fifo)){ret=-ENODEV;goto unlock;}
 n=min_t(size_t,n,16384);tmp=kmalloc(n,GFP_KERNEL);if(!tmp){ret=-ENOMEM;goto unlock;}
 spin_lock_irqsave(&rxlock,flags);
 if((unsigned long)f->private_data!=usb_generation){spin_unlock_irqrestore(&rxlock,flags);kfree(tmp);ret=-ENODEV;goto unlock;}
 copied=kfifo_out(&fifo,tmp,n);spin_unlock_irqrestore(&rxlock,flags);
 ret=copy_to_user(buf,tmp,copied)?-EFAULT:copied;kfree(tmp);
unlock:mutex_unlock(&readlock);return ret;
}
static ssize_t aa_write(struct file *f,const char __user *buf,size_t n,loff_t *off){
 struct usb_request *r;DECLARE_COMPLETION_ONSTACK(done);int ret;
 if(!n)return 0;if(n>65536)return -EMSGSIZE;
 ret=mutex_lock_interruptible(&writelock);if(ret)return ret;
 if(!READ_ONCE(online) || (unsigned long)f->private_data!=READ_ONCE(usb_generation)){ret=-ENODEV;goto unlock;}
 r=usb_ep_alloc_request(epin,GFP_KERNEL);if(!r){ret=-ENOMEM;goto unlock;}
 r->buf=memdup_user(buf,n);if(IS_ERR(r->buf)){ret=PTR_ERR(r->buf);goto free_req;}
 r->length=n;r->complete=tx_done;r->context=&done;
 ret=usb_ep_queue(epin,r,GFP_KERNEL);
 if(!ret){wait_for_completion(&done);ret=r->status?r->status:r->actual;if(ret>0)tx_bytes+=ret;}
 kfree(r->buf);
free_req:usb_ep_free_request(epin,r);
unlock:mutex_unlock(&writelock);return ret;
}
static const struct file_operations fops={.owner=THIS_MODULE,.open=aa_open,.read=aa_read,.write=aa_write,.llseek=noop_llseek};
static struct miscdevice misc={.minor=MISC_DYNAMIC_MINOR,.name="aa_bulk",.fops=&fops,.mode=0600};
static struct usb_interface_descriptor intf={.bLength=USB_DT_INTERFACE_SIZE,.bDescriptorType=USB_DT_INTERFACE,.bNumEndpoints=2,.bInterfaceClass=USB_CLASS_VENDOR_SPEC,.bInterfaceSubClass=0xff,.bInterfaceProtocol=0};
static struct usb_endpoint_descriptor fsin={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_IN,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(64)};
static struct usb_endpoint_descriptor fsout={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_OUT,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(64)};
static struct usb_endpoint_descriptor hsin={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_IN,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(512)};
static struct usb_endpoint_descriptor hsout={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_OUT,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(512)};
static struct usb_descriptor_header *fs[]={ (void*)&intf,(void*)&fsin,(void*)&fsout,NULL};
static struct usb_descriptor_header *hs[]={ (void*)&intf,(void*)&hsin,(void*)&hsout,NULL};
static struct usb_interface_descriptor mtp_intf={.bLength=USB_DT_INTERFACE_SIZE,.bDescriptorType=USB_DT_INTERFACE,.bNumEndpoints=3,.bInterfaceClass=USB_CLASS_STILL_IMAGE,.bInterfaceSubClass=1,.bInterfaceProtocol=1};
static struct usb_endpoint_descriptor fsnotify={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_IN,.bmAttributes=USB_ENDPOINT_XFER_INT,.wMaxPacketSize=cpu_to_le16(16),.bInterval=6};
static struct usb_endpoint_descriptor hsnotify={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_IN,.bmAttributes=USB_ENDPOINT_XFER_INT,.wMaxPacketSize=cpu_to_le16(16),.bInterval=9};
static struct usb_descriptor_header *mtp_fs[]={ (void*)&mtp_intf,(void*)&fsin,(void*)&fsout,(void*)&fsnotify,NULL};
static struct usb_descriptor_header *mtp_hs[]={ (void*)&mtp_intf,(void*)&hsin,(void*)&hsout,(void*)&hsnotify,NULL};
static void disable(struct usb_function *f){
 unsigned long flags;
 cancel_delayed_work(&mtp_timeout_work);
 spin_lock_irqsave(&rxlock,flags);usb_generation++;kfifo_reset(&fifo);WRITE_ONCE(disconnected,true);WRITE_ONCE(online,false);spin_unlock_irqrestore(&rxlock,flags);
 wake_up_interruptible(&rxwait);if(notification_online){usb_ep_disable(epnotify);notification_online=false;}if(epout)usb_ep_disable(epout);if(epin)usb_ep_disable(epin);
}
static int set_alt(struct usb_function *f,unsigned intf_id,unsigned alt){
 int i,ret;
 if(!READ_ONCE(accessory) && !legacy_mtp)return 0;
 if(READ_ONCE(online))disable(f);
 ret=config_ep_by_speed(f->config->cdev->gadget,f,epin);if(ret)return ret;
 ret=config_ep_by_speed(f->config->cdev->gadget,f,epout);if(ret)return ret;
 ret=usb_ep_enable(epin);if(ret)return ret;
 ret=usb_ep_enable(epout);if(ret){usb_ep_disable(epin);return ret;}
 if(legacy_mtp && !READ_ONCE(accessory)){
  ret=config_ep_by_speed(f->config->cdev->gadget,f,epnotify);if(ret){usb_ep_disable(epout);usb_ep_disable(epin);return ret;}
  ret=usb_ep_enable(epnotify);if(ret){usb_ep_disable(epout);usb_ep_disable(epin);return ret;}notification_online=true;
 }
 WRITE_ONCE(disconnected,false);WRITE_ONCE(overflow,false);WRITE_ONCE(online,true);
 for(i=0;i<DEPTH;i++){ret=usb_ep_queue(epout,rx[i],GFP_ATOMIC);if(ret){disable(f);return ret;}}
 pr_info("aa_bulk: online, %d RX requests queued personality=%s\n",DEPTH,READ_ONCE(accessory)?"accessory":"mtp");
 if(legacy_mtp && !READ_ONCE(accessory) && mtp_fallback_ms){
  schedule_delayed_work(&mtp_timeout_work,msecs_to_jiffies(mtp_fallback_ms));
  pr_info("aa_bulk: MTP fallback armed after %u ms\n",mtp_fallback_ms);
 }
 wake_up_interruptible(&rxwait);return 0;
}
static int func_bind(struct usb_configuration *c,struct usb_function *f){
 int i,id=usb_interface_id(c,f);if(id<0)return id;intf.bInterfaceNumber=id;mtp_intf.bInterfaceNumber=id;
 epin=usb_ep_autoconfig(c->cdev->gadget,&fsin);epout=usb_ep_autoconfig(c->cdev->gadget,&fsout);if(!epin||!epout)return -ENODEV;
 hsin.bEndpointAddress=fsin.bEndpointAddress;hsout.bEndpointAddress=fsout.bEndpointAddress;
 if(legacy_mtp){epnotify=usb_ep_autoconfig(c->cdev->gadget,&fsnotify);if(!epnotify)return -ENODEV;hsnotify.bEndpointAddress=fsnotify.bEndpointAddress;}
 for(i=0;i<DEPTH;i++){
  rx[i]=usb_ep_alloc_request(epout,GFP_KERNEL);if(!rx[i])return -ENOMEM;
  rx[i]->buf=kmalloc(CHUNK,GFP_KERNEL);if(!rx[i]->buf)return -ENOMEM;
  rx[i]->length=CHUNK;rx[i]->complete=rx_done;
 }
 return usb_assign_descriptors(f,legacy_mtp?mtp_fs:fs,legacy_mtp?mtp_hs:hs,NULL,NULL);
}
static void func_unbind(struct usb_configuration *c,struct usb_function *f){int i;for(i=0;i<DEPTH;i++)if(rx[i]){kfree(rx[i]->buf);usb_ep_free_request(epout,rx[i]);rx[i]=NULL;}usb_free_all_descriptors(f);}
/* AOA endpoint-zero negotiation works before SET_CONFIGURATION as well. */
static bool aoa_match(struct usb_function *f,const struct usb_ctrlrequest *ctrl,bool config0){
 if((ctrl->bRequestType & USB_TYPE_MASK)==USB_TYPE_VENDOR && vendor_probes++<64)
  pr_info("aa_bulk: vendor SETUP type=%02x request=%u value=%u index=%u length=%u config0=%u\n",
          ctrl->bRequestType,ctrl->bRequest,le16_to_cpu(ctrl->wValue),
          le16_to_cpu(ctrl->wIndex),le16_to_cpu(ctrl->wLength),config0);
 if(legacy_mtp && !READ_ONCE(accessory) &&
    (ctrl->bRequestType & (USB_TYPE_MASK|USB_RECIP_MASK))==(USB_TYPE_CLASS|USB_RECIP_INTERFACE) &&
    le16_to_cpu(ctrl->wIndex)==mtp_intf.bInterfaceNumber)return true;
 return (ctrl->bRequestType & (USB_TYPE_MASK|USB_RECIP_MASK))==(USB_TYPE_VENDOR|USB_RECIP_DEVICE)
  && ctrl->bRequest>=51 && ctrl->bRequest<=58;
}
static void aoa_complete(struct usb_ep *ep,struct usb_request *req){
 aoa_completed++;aoa_last_status=req->status;if(req->status)aoa_errors++;
 pr_info("aa_bulk: EP0 complete status=%d actual=%u expected=%u start=%u\n",
         req->status,req->actual,req->length,req->context==(void *)1);
 if(!req->status && req->context==(void *)1){
  cancel_delayed_work(&mtp_timeout_work);
  schedule_delayed_work(&aoa_work,msecs_to_jiffies(50));
 }
}
static int aoa_setup(struct usb_function *f,const struct usb_ctrlrequest *ctrl){
 struct usb_composite_dev *cdev=f->config->cdev;
 struct usb_request *req=cdev->req;
 u16 length=le16_to_cpu(ctrl->wLength),index=le16_to_cpu(ctrl->wIndex);
 u8 type=ctrl->bRequestType;
 int count;
 if(legacy_mtp && !READ_ONCE(accessory) && (type & USB_TYPE_MASK)==USB_TYPE_CLASS){
  if(le16_to_cpu(ctrl->wValue))return -EOPNOTSUPP;
  req->context=NULL;req->complete=aoa_complete;req->zero=0;
  if(type==0xa1 && ctrl->bRequest==0x67 && length>=4){
   ((u8 *)req->buf)[0]=4;((u8 *)req->buf)[1]=0;((u8 *)req->buf)[2]=1;((u8 *)req->buf)[3]=0x20;req->length=4;
  }else if(type==0x21 && ctrl->bRequest==0x64 && length==6){req->length=6;
  }else {pr_info("aa_bulk: unsupported MTP class request=%u length=%u\n",ctrl->bRequest,length);return -EOPNOTSUPP;}
  pr_info("aa_bulk: MTP class request=%u length=%u\n",ctrl->bRequest,length);
  return usb_ep_queue(cdev->gadget->ep0,req,GFP_ATOMIC);
 }
 count=aa_aoa_length(type,ctrl->bRequest,le16_to_cpu(ctrl->wValue),index,length);
 if(count<0){
  aoa_unsupported++;
  pr_info("aa_bulk: unsupported AOA type=%02x request=%u value=%u index=%u length=%u\n",
          type,ctrl->bRequest,le16_to_cpu(ctrl->wValue),index,length);
  return -EOPNOTSUPP;
 }
 req->context=NULL;req->complete=aoa_complete;req->zero=0;
 switch(ctrl->bRequest){
 case 51:
  aa_aoa_protocol_reply(READ_ONCE(aoa_protocol),req->buf);req->length=2;pr_info("aa_bulk: GET_PROTOCOL reply=%02x00 (AA transport profile %u)\n",((u8 *)req->buf)[0],READ_ONCE(aoa_protocol));break;
 case 52:
  /* Android Auto owns the projection; accessory identification is not persisted. */
  req->length=length;break;
 case 53:
  req->length=0;req->context=(void *)1;break;
 case 58:
  /* Only mode 0: AA audio is carried inside the AA protocol, not USB Audio. */
  req->length=0;break;
 default:return -EOPNOTSUPP;
 }
 aoa_requests++;pr_info("aa_bulk: AOA request=%u index=%u length=%u\n",ctrl->bRequest,index,length);
 count=usb_ep_queue(cdev->gadget->ep0,req,GFP_ATOMIC);
 if(count){aoa_errors++;aoa_last_status=count;pr_err("aa_bulk: EP0 queue failed %d\n",count);}
 return count;
}
static void mtp_timeout(struct work_struct *work){
 if(legacy_mtp && aoa_cdev && !READ_ONCE(accessory) && READ_ONCE(online)){
  pr_warn("aa_bulk: no ACCESSORY_START after MTP discovery; compatibility fallback to accessory\n");
  schedule_delayed_work(&aoa_work,0);
 }
}
static void aoa_switch(struct work_struct *work){
 struct usb_composite_dev *cdev=aoa_cdev;
 if(!cdev || READ_ONCE(accessory))return;
 usb_gadget_disconnect(cdev->gadget);
 msleep(250);
 if(legacy_mtp && !READ_ONCE(accessory)){
  unsigned long flags;int ret;
  spin_lock_irqsave(&rxlock,flags);kfifo_reset(&fifo);spin_unlock_irqrestore(&rxlock,flags);
  usb_free_all_descriptors(&function);
  ret=usb_assign_descriptors(&function,fs,hs,NULL,NULL);
  if(ret){pr_err("aa_bulk: accessory descriptor switch failed %d\n",ret);return;}
 }
 cdev->desc.idVendor=cpu_to_le16(0x18d1);
 cdev->desc.idProduct=cpu_to_le16(0x2d00);
 WRITE_ONCE(accessory,true);
 usb_gadget_connect(cdev->gadget);
 pr_info("aa_bulk: AOA accessory mode re-enumeration\n");
}
static struct usb_function function={.name="aa_bulk",.bind=func_bind,.unbind=func_unbind,.set_alt=set_alt,.disable=disable,.req_match=aoa_match,.setup=aoa_setup};
static int config_bind(struct usb_configuration *c){return usb_add_function(c,&function);}
static struct usb_configuration config={.label="Android Auto",.bConfigurationValue=1,.bmAttributes=USB_CONFIG_ATT_ONE,.MaxPower=250};
static struct usb_device_descriptor device={.bLength=sizeof device,.bDescriptorType=USB_DT_DEVICE,.bcdUSB=cpu_to_le16(0x200),.idVendor=cpu_to_le16(0x18d1),.idProduct=cpu_to_le16(0x2d00),.bcdDevice=cpu_to_le16(0x100),.bNumConfigurations=1};
static struct usb_string strings[]={{.s="CarFluxoid"},{.s="CarFluxoid"},{.s="CATPLAY_AA_LAB_KERNEL_QUEUE"},{.s=AA_ACCESSORY_INTERFACE_NAME},{}};
static struct usb_gadget_strings lang={.language=0x409,.strings=strings};
static struct usb_gadget_strings *allstrings[]={&lang,NULL};
static int bind(struct usb_composite_dev *c){int ret;aoa_cdev=c;accessory=!pre_accessory;if(pre_accessory)c->desc.idProduct=cpu_to_le16(0x4ee1);ret=usb_string_ids_tab(c,strings);if(ret<0)return ret;device.iManufacturer=strings[0].id;device.iProduct=strings[1].id;device.iSerialNumber=strings[2].id;intf.iInterface=strings[3].id;return usb_add_config(c,&config,config_bind);}
static struct usb_composite_driver driver={.name="aa_bulk",.dev=&device,.strings=allstrings,.max_speed=USB_SPEED_HIGH,.bind=bind};
static int __init init(void){int ret;if(legacy_mtp && !pre_accessory)return -EINVAL;if(mtp_fallback_ms && (!legacy_mtp || mtp_fallback_ms<1000 || mtp_fallback_ms>60000))return -EINVAL;INIT_KFIFO(fifo);ret=misc_register(&misc);if(ret)return ret;ret=usb_composite_probe(&driver);if(ret)misc_deregister(&misc);return ret;}
static void __exit fini(void){cancel_delayed_work_sync(&mtp_timeout_work);cancel_delayed_work_sync(&aoa_work);usb_composite_unregister(&driver);misc_deregister(&misc);}
module_init(init);module_exit(fini);MODULE_LICENSE("GPL");MODULE_DESCRIPTION("Experimental Android Auto gadget with prequeued RX requests");
