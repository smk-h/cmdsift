#include "calc.h"

int main(void)
{
    /* [Warning] 未使用变量 unused_value：-Wall 报 unused variable */
    int unused_value = 42;

    int sum = calc_add(3, 4);
    int diff = calc_sub(10, 6);
    int product = calc_mul(5, 5);
    int len = utils_get_length("hello");

    /* [Error] 类型不匹配：calc_add 第二个参数传了字符串，-Wint-conversion 报
     *         passing argument 2 of 'calc_add' makes integer from pointer */
    int bad = calc_add(sum, "not-a-number");

    /* [Warning] 隐式声明 printf（未 include <stdio.h>） */
    printf("sum=%d diff=%d product=%d len=%d bad=%d\n",
           sum, diff, product, len, bad);

    return 0;
}
