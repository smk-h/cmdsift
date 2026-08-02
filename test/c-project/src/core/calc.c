#include "calc.h"

int calc_add(int a, int b)
{
    return a + b;
}

int calc_sub(int a, int b)
{
    return a - b;
}

int calc_mul(int a, int b)
{
    /* [Warning] 未使用的变量 result：-Wall 会报 unused variable */
    int result = a * b;

    /* [Error] 返回了未定义的符号 nonexistent_symbol，链接阶段报
     *         undefined reference to 'nonexistent_symbol' */
    return nonexistent_symbol;
}
