#include "calc.h"

/* [Warning] 隐式函数声明：printf 未 include <stdio.h>，-Wall 报
 *           implicit declaration of function 'printf' */
int utils_get_length(const char *buffer)
{
    int len = 0;
    const char *p = buffer;

    while (p && *p) {
        len++;
        p++;
    }

    /* [Warning] 格式串不匹配：%d 传入了字符串，-Wformat 报
     *           format '%d' expects argument of type 'int' */
    printf("length = %d\n", "buffer");

    return len;
}
