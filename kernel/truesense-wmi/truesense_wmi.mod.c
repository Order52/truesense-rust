#include <linux/module.h>
#include <linux/export-internal.h>
#include <linux/compiler.h>

MODULE_INFO(name, KBUILD_MODNAME);

__visible struct module __this_module
__section(".gnu.linkonce.this_module") = {
	.name = KBUILD_MODNAME,
	.init = init_module,
#ifdef CONFIG_MODULE_UNLOAD
	.exit = cleanup_module,
#endif
	.arch = MODULE_ARCH_INIT,
};


MODULE_INFO(depends, "wmi");

MODULE_ALIAS("dmi*:svn*Acer*:pn*PredatorPH16-71*:");
MODULE_ALIAS("wmi:7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56");
MODULE_ALIAS("wmi:61EF69EA-865C-4BC3-A502-A0DEBA0CB531");
MODULE_ALIAS("wmi:79772EC5-04B1-4BFD-843C-61E7F77B6CC9");

MODULE_INFO(srcversion, "C5F0DDF14F0EA56706D2630");
