#include <sys/socket.h>
#include <netinet/in.h>
#include <string.h>
#include <unistd.h>
#include <stdio.h>

static const char resp[] =
    "HTTP/1.1 200 OK\r\n"
    "Content-Type: text/html\r\n"
    "Content-Length: 53\r\n"
    "Connection: close\r\n"
    "\r\n"
    "<html><body><h1>hello from user socket!</h1></body></html>";

int main() {
    int s = socket(AF_INET, SOCK_STREAM, 0);
    if (s < 0) { write(1, "socket fail\n", 12); return 1; }

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(80);
    addr.sin_addr.s_addr = 0;  // INADDR_ANY

    if (bind(s, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        write(1, "bind fail\n", 9); return 1;
    }
    if (listen(s, 8) < 0) {
        write(1, "listen fail\n", 12); return 1;
    }
    write(1, "listening on 80\n", 16);

    while (1) {
        int c = accept(s, 0, 0);
        if (c < 0) { continue; }
        char buf[1024];
        int n = read(c, buf, sizeof(buf));
        (void)n;
        write(c, resp, sizeof(resp)-1);
        close(c);
    }
    return 0;
}
