// SPDX-License-Identifier: GPL-2.0
#include <linux/module.h>
#include <linux/usb/composite.h>
#include <linux/miscdevice.h>
#include <linux/kfifo.h>
#include <linux/wait.h>
#include <linux/completion.h>
#include <linux/uaccess.h>
#include <linux/mutex.h>
#define DEPTH 8
#define CHUNK 4096
static struct usb_ep *epin,*epout;
static struct usb_request *rx[DEPTH];
static DECLARE_KFIFO(fifo,u8,131072);
static DEFINE_SPINLOCK(rxlock);
static DECLARE_WAIT_QUEUE_HEAD(rxwait);
static DEFINE_MUTEX(readlock);
static DEFINE_MUTEX(writelock);
static bool online,overflow,disconnected;
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
 ret=wait_event_interruptible(rxwait,!kfifo_is_empty(&fifo)||READ_ONCE(disconnected)||READ_ONCE(overflow));
 if(ret)goto unlock;
 if(READ_ONCE(overflow)){ret=-EOVERFLOW;goto unlock;}
 if(kfifo_is_empty(&fifo)){ret=-ENODEV;goto unlock;}
 n=min_t(size_t,n,16384);tmp=kmalloc(n,GFP_KERNEL);if(!tmp){ret=-ENOMEM;goto unlock;}
 spin_lock_irqsave(&rxlock,flags);copied=kfifo_out(&fifo,tmp,n);spin_unlock_irqrestore(&rxlock,flags);
 ret=copy_to_user(buf,tmp,copied)?-EFAULT:copied;kfree(tmp);
unlock:mutex_unlock(&readlock);return ret;
}
static ssize_t aa_write(struct file *f,const char __user *buf,size_t n,loff_t *off){
 struct usb_request *r;DECLARE_COMPLETION_ONSTACK(done);int ret;
 if(!n)return 0;if(n>65536)return -EMSGSIZE;
 ret=mutex_lock_interruptible(&writelock);if(ret)return ret;
 if(!READ_ONCE(online)){ret=-ENODEV;goto unlock;}
 r=usb_ep_alloc_request(epin,GFP_KERNEL);if(!r){ret=-ENOMEM;goto unlock;}
 r->buf=memdup_user(buf,n);if(IS_ERR(r->buf)){ret=PTR_ERR(r->buf);goto free_req;}
 r->length=n;r->complete=tx_done;r->context=&done;
 ret=usb_ep_queue(epin,r,GFP_KERNEL);
 if(!ret){wait_for_completion(&done);ret=r->status?r->status:r->actual;if(ret>0)tx_bytes+=ret;}
 kfree(r->buf);
free_req:usb_ep_free_request(epin,r);
unlock:mutex_unlock(&writelock);return ret;
}
static const struct file_operations fops={.owner=THIS_MODULE,.read=aa_read,.write=aa_write,.llseek=noop_llseek};
static struct miscdevice misc={.minor=MISC_DYNAMIC_MINOR,.name="aa_bulk",.fops=&fops,.mode=0600};
static struct usb_interface_descriptor intf={.bLength=USB_DT_INTERFACE_SIZE,.bDescriptorType=USB_DT_INTERFACE,.bNumEndpoints=2,.bInterfaceClass=USB_CLASS_VENDOR_SPEC,.bInterfaceSubClass=0xff,.bInterfaceProtocol=0};
static struct usb_endpoint_descriptor fsin={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_IN,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(64)};
static struct usb_endpoint_descriptor fsout={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_OUT,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(64)};
static struct usb_endpoint_descriptor hsin={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_IN,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(512)};
static struct usb_endpoint_descriptor hsout={.bLength=USB_DT_ENDPOINT_SIZE,.bDescriptorType=USB_DT_ENDPOINT,.bEndpointAddress=USB_DIR_OUT,.bmAttributes=USB_ENDPOINT_XFER_BULK,.wMaxPacketSize=cpu_to_le16(512)};
static struct usb_descriptor_header *fs[]={ (void*)&intf,(void*)&fsin,(void*)&fsout,NULL};
static struct usb_descriptor_header *hs[]={ (void*)&intf,(void*)&hsin,(void*)&hsout,NULL};
static void disable(struct usb_function *f){
 WRITE_ONCE(disconnected,true);WRITE_ONCE(online,false);wake_up_interruptible(&rxwait);if(epout)usb_ep_disable(epout);if(epin)usb_ep_disable(epin);
}
static int set_alt(struct usb_function *f,unsigned intf_id,unsigned alt){
 int i,ret;
 ret=config_ep_by_speed(f->config->cdev->gadget,f,epin);if(ret)return ret;
 ret=config_ep_by_speed(f->config->cdev->gadget,f,epout);if(ret)return ret;
 ret=usb_ep_enable(epin);if(ret)return ret;
 ret=usb_ep_enable(epout);if(ret){usb_ep_disable(epin);return ret;}
 WRITE_ONCE(disconnected,false);WRITE_ONCE(overflow,false);WRITE_ONCE(online,true);
 for(i=0;i<DEPTH;i++){ret=usb_ep_queue(epout,rx[i],GFP_ATOMIC);if(ret){disable(f);return ret;}}
 pr_info("aa_bulk: online, %d RX requests queued\n",DEPTH);wake_up_interruptible(&rxwait);return 0;
}
static int func_bind(struct usb_configuration *c,struct usb_function *f){
 int i,id=usb_interface_id(c,f);if(id<0)return id;intf.bInterfaceNumber=id;
 epin=usb_ep_autoconfig(c->cdev->gadget,&fsin);epout=usb_ep_autoconfig(c->cdev->gadget,&fsout);if(!epin||!epout)return -ENODEV;
 hsin.bEndpointAddress=fsin.bEndpointAddress;hsout.bEndpointAddress=fsout.bEndpointAddress;
 for(i=0;i<DEPTH;i++){
  rx[i]=usb_ep_alloc_request(epout,GFP_KERNEL);if(!rx[i])return -ENOMEM;
  rx[i]->buf=kmalloc(CHUNK,GFP_KERNEL);if(!rx[i]->buf)return -ENOMEM;
  rx[i]->length=CHUNK;rx[i]->complete=rx_done;
 }
 return usb_assign_descriptors(f,fs,hs,NULL,NULL);
}
static void func_unbind(struct usb_configuration *c,struct usb_function *f){int i;for(i=0;i<DEPTH;i++)if(rx[i]){kfree(rx[i]->buf);usb_ep_free_request(epout,rx[i]);rx[i]=NULL;}usb_free_all_descriptors(f);}
static struct usb_function function={.name="aa_bulk",.bind=func_bind,.unbind=func_unbind,.set_alt=set_alt,.disable=disable};
static int config_bind(struct usb_configuration *c){return usb_add_function(c,&function);}
static struct usb_configuration config={.label="Android Auto",.bConfigurationValue=1,.bmAttributes=USB_CONFIG_ATT_ONE,.MaxPower=250};
static struct usb_device_descriptor device={.bLength=sizeof device,.bDescriptorType=USB_DT_DEVICE,.bcdUSB=cpu_to_le16(0x200),.idVendor=cpu_to_le16(0x18d1),.idProduct=cpu_to_le16(0x2d00),.bcdDevice=cpu_to_le16(0x100),.bNumConfigurations=1};
static struct usb_string strings[]={{.s="CarFluxoid"},{.s="CarFluxoid"},{.s="CATPLAY_AA_LAB_KERNEL_QUEUE"},{}};
static struct usb_gadget_strings lang={.language=0x409,.strings=strings};
static struct usb_gadget_strings *allstrings[]={&lang,NULL};
static int bind(struct usb_composite_dev *c){int ret=usb_string_ids_tab(c,strings);if(ret<0)return ret;device.iManufacturer=strings[0].id;device.iProduct=strings[1].id;device.iSerialNumber=strings[2].id;return usb_add_config(c,&config,config_bind);}
static struct usb_composite_driver driver={.name="aa_bulk",.dev=&device,.strings=allstrings,.max_speed=USB_SPEED_HIGH,.bind=bind};
static int __init init(void){int ret;INIT_KFIFO(fifo);ret=misc_register(&misc);if(ret)return ret;ret=usb_composite_probe(&driver);if(ret)misc_deregister(&misc);return ret;}
static void __exit fini(void){usb_composite_unregister(&driver);misc_deregister(&misc);}
module_init(init);module_exit(fini);MODULE_LICENSE("GPL");MODULE_DESCRIPTION("Experimental Android Auto gadget with prequeued RX requests");
