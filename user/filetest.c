#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>

int main() {
    int fd = open("/etc/hostname", O_RDONLY);
    if (fd < 0) {
        write(1, "open failed\n", 11);
        return 1;
    }
    char buf[64];
    int n = read(fd, buf, sizeof(buf) - 1);
    if (n > 0) {
        buf[n] = 0;
        write(1, "hostname: ", 10);
        write(1, buf, n);
    }
    close(fd);

    struct stat st;
    if (stat("/index.html", &st) == 0) {
        printf("index.html size = %ld\n", (long)st.st_size);
    } else {
        write(1, "stat failed\n", 12);
    }
    return 0;
}
