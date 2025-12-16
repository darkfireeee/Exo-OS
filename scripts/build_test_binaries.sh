#!/bin/bash
# Build test binaries for Phase 1 validation
# Uses musl-gcc for static Linux binaries compatible with Exo-OS

set -e

echo "🔨 Building Phase 1 Test Binaries..."
echo ""

# Check for musl-gcc
if ! command -v musl-gcc &> /dev/null; then
    echo "❌ musl-gcc not found!"
    echo "Install with: sudo apt-get install musl-tools"
    exit 1
fi

# Create userland directory if it doesn't exist
mkdir -p userland/bin

# Build hello.c
echo "📝 Building hello..."
cat > userland/hello.c << 'EOF'
#include <stdio.h>
#include <unistd.h>

int main() {
    printf("Hello from Exo-OS userspace!\n");
    printf("PID: %d\n", getpid());
    return 0;
}
EOF

musl-gcc -static -o userland/bin/hello userland/hello.c
echo "✅ userland/bin/hello created"

# Build test_args.c
echo "📝 Building test_args..."
cat > userland/test_args.c << 'EOF'
#include <stdio.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    printf("test_args running\n");
    printf("argc: %d\n", argc);
    printf("PID: %d\n", getpid());
    
    for (int i = 0; i < argc; i++) {
        printf("argv[%d]: %s\n", i, argv[i]);
    }
    
    return 0;
}
EOF

musl-gcc -static -o userland/bin/test_args userland/test_args.c
echo "✅ userland/bin/test_args created"

# Build test_fork.c
echo "📝 Building test_fork..."
cat > userland/test_fork.c << 'EOF'
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

int main() {
    printf("Parent PID: %d\n", getpid());
    
    pid_t pid = fork();
    
    if (pid == 0) {
        // Child
        printf("Child PID: %d, Parent PID: %d\n", getpid(), getppid());
        return 42;
    } else if (pid > 0) {
        // Parent
        printf("Parent created child with PID: %d\n", pid);
        int status;
        wait4(pid, &status, 0, NULL);
        printf("Child exited with status: %d\n", WEXITSTATUS(status));
    } else {
        perror("fork");
        return 1;
    }
    
    return 0;
}
EOF

musl-gcc -static -o userland/bin/test_fork userland/test_fork.c
echo "✅ userland/bin/test_fork created"

# Build test_pipe.c
echo "📝 Building test_pipe..."
cat > userland/test_pipe.c << 'EOF'
#include <stdio.h>
#include <unistd.h>
#include <string.h>
#include <sys/wait.h>

int main() {
    int pipefd[2];
    
    if (pipe(pipefd) == -1) {
        perror("pipe");
        return 1;
    }
    
    pid_t pid = fork();
    
    if (pid == 0) {
        // Child: writer
        close(pipefd[0]); // Close read end
        const char *msg = "Hello from child via pipe!";
        write(pipefd[1], msg, strlen(msg));
        close(pipefd[1]);
        return 0;
    } else if (pid > 0) {
        // Parent: reader
        close(pipefd[1]); // Close write end
        char buf[100];
        ssize_t n = read(pipefd[0], buf, sizeof(buf));
        if (n > 0) {
            buf[n] = '\0';
            printf("Parent received: %s\n", buf);
        }
        close(pipefd[0]);
        wait4(pid, NULL, 0, NULL);
    } else {
        perror("fork");
        return 1;
    }
    
    return 0;
}
EOF

musl-gcc -static -o userland/bin/test_pipe userland/test_pipe.c
echo "✅ userland/bin/test_pipe created"

# Build test_file_io.c
echo "📝 Building test_file_io..."
cat > userland/test_file_io.c << 'EOF'
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>
#include <sys/stat.h>

int main() {
    const char *path = "/tmp/test_file.txt";
    const char *msg = "Hello from Exo-OS file I/O!";
    
    // Write
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) {
        perror("open for write");
        return 1;
    }
    write(fd, msg, strlen(msg));
    close(fd);
    printf("Wrote to %s\n", path);
    
    // Read back
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        perror("open for read");
        return 1;
    }
    char buf[100];
    ssize_t n = read(fd, buf, sizeof(buf));
    buf[n] = '\0';
    printf("Read: %s\n", buf);
    close(fd);
    
    // Stat
    struct stat st;
    if (stat(path, &st) == 0) {
        printf("File size: %ld bytes\n", st.st_size);
    }
    
    // Delete
    unlink(path);
    printf("File deleted\n");
    
    return 0;
}
EOF

musl-gcc -static -o userland/bin/test_file_io userland/test_file_io.c
echo "✅ userland/bin/test_file_io created"

# Summary
echo ""
echo "✅ All test binaries built successfully!"
echo ""
echo "📦 Binaries created:"
ls -lh userland/bin/
echo ""
echo "🚀 Next steps:"
echo "1. Add binaries to tmpfs during kernel init"
echo "2. Run tests: test_exec_hello(), test_fork_exec_wait()"
echo "3. Validate Phase 1 complete"
