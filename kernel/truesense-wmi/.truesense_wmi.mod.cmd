savedcmd_truesense_wmi.mod := printf '%s\n'   truesense_wmi.o | awk '!x[$$0]++ { print("./"$$0) }' > truesense_wmi.mod
