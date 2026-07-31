#include <unistd.h>
#include <fcntl.h>
int main(void) {
    const char *msg = "hello from JiegeOS userland!\n";
    write(1, msg, 30);
    return 0;
}
