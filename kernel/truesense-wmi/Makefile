obj-m += truesense_wmi.o

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build
# a kernel built with clang (CachyOS) needs the module built with clang too; gcc-built kernels use gcc
LLVM_FLAG := $(if $(shell grep -s '^CONFIG_CC_IS_CLANG=y' $(KDIR)/.config),LLVM=1,)

all:
	$(MAKE) -C $(KDIR) M=$(CURDIR) $(LLVM_FLAG) modules

clean:
	$(MAKE) -C $(KDIR) M=$(CURDIR) $(LLVM_FLAG) clean
