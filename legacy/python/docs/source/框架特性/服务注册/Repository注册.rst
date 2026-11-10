Repository注册
_____________

注意事项：

- 自定义的UserRepository类需要继承RepositoryBase父类。
- 需要使用@Repository来装饰自定义的UserRepository类，目的是将自定义的UserRepository注册到系统当中，方便Service层注入。
- 自定义的UserRepository类会注册为转换成小写以下划线分割的实例名，所以类名需要使用开头字母大写的驼峰命名法。
- 下面代码中的mysql属于已实现的mysql数据源，对Datasource注册及扩展感兴趣的同学请看 :doc:`数据源 <../数据源/index>`。
- 注意类方法比如get_all_user，没有self参数。原因是在装饰类方法之前，该类实例并未创建，无法获取到self。
  这是因为Autowired是使用类来实现的装饰器，这是类装饰器的弊端。如果有更好的办法请联系我，欢迎提交PR。
  因此解决办法就是当作类静态方法来处理，直接删掉self，以便sevice层调用。
.. code-block:: python
    :linenos:

    from backend.core.component.autowired import Autowired
    from backend.core.component.repository import Repository
    from backend.model.Repository import RepositoryBase

    @Repository
    class UserRepository(RepositoryBase):
        @Autowired
        def mysql(self):
            pass

        @mysql.execute
        def get_all_user(results):
            sql = """
            SELECT u1.uid,
                u1.username,
                u1.phone,
                u1.email,
                u1.user_role,
                u2.username AS create_by,
                u1.create_time,
                u1.last_login_time,
                u1.status
            FROM user AS u1
            LEFT JOIN user AS u2
                ON u1.create_by = u2.uid
            """
            return sql
