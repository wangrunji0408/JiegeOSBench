'''
apport package hook for nginx packages

Copyright (c) 2015, Thomas Ward <teward@ubuntu.com>
'''

import apport.hookutils
import os
import subprocess

def add_info(report, ui):
    if (report['Package'].split()[0] != 'nginx-common'
        and report['ProblemType'] == 'Package'
        and os.path.isdir('/run/systemd/system')):
        report['Journalctl_Nginx.txt'] = apport.hookutils.command_output(
            ['journalctl', '-xe', '--unit=nginx.service'])
        report['SystemctlStatusFull_Nginx.txt'] = subprocess.Popen(
            ['systemctl', '-l', 'status', 'nginx.service'],
            stdout=subprocess.PIPE).communicate()[0]
