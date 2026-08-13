#include <unistd.h>
#include <stdio.h>
int main(int argc, char** argv) {
    printf("Hello from user mode! argc=%d\n", argc);
    for (int i = 0; i < argc; i++) printf("argv[%d]=%s\n", i, argv[i]);
    fflush(stdout);
    return 0;
}
