################################################################################
#
# aa-bulk — CarFluxoid USB AOA gadget kernel module
#
################################################################################

AA_BULK_VERSION = local
AA_BULK_SITE = $(BR2_EXTERNAL_CARFLUXOID_PATH)/../bridge-aa/kernel
AA_BULK_SITE_METHOD = local
AA_BULK_LICENSE = GPL-2.0

$(eval $(kernel-module))
$(eval $(generic-package))
