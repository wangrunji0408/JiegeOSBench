#include <unistd.h>
#include <string.h>

int main(int argc, char **argv) {
    const char *msg = "hello from real linux binary!\n";
    write(1, msg, strlen(msg));
    _exit(0);
    return 0;
}
