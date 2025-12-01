#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    printf("Hello from musl on Exo-OS!\n");
    printf("This is POSIX-X in action!\n");
    
    // Test write() directly
    const char *msg = "Direct write() syscall test\n";
    write(1, msg, 28);
    
    printf("argc = %d\n", argc);
    if (argc > 0) {
        printf("argv[0] = %s\n", argv[0]);
    }
    
    return 0;
}
