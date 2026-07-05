#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>
#include <time.h>

int main(int argc, char **argv) {
    printf("hello from printf, pid=%d argc=%d\n", getpid(), argc);
    char *p = malloc(1024);
    if (p) {
        strcpy(p, "dynamic string from malloc");
        printf("malloc ok: %s\n", p);
        free(p);
    } else {
        printf("malloc failed\n");
    }
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    printf("time: %ld.%09ld\n", (long)ts.tv_sec, (long)ts.tv_nsec);
    write(1, "raw write ok\n", 13);
    return 0;
}
